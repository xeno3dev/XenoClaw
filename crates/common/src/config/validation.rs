//! Configuration validation for the VPS AI Agent Platform.
//!
//! Validates all configuration settings and collects ALL errors
//! (does not stop at the first invalid setting).

use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

use super::models::{PlatformConfig, ProviderType};

/// A single validation error for a specific setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// The setting name (dot-separated path, e.g., "llm.providers[0].timeout_seconds").
    pub setting: String,
    /// Human-readable reason why the setting is invalid.
    pub reason: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.setting, self.reason)
    }
}

/// Collection of all validation errors found in a configuration.
#[derive(Debug, Clone)]
pub struct ConfigValidationError {
    /// All validation errors found.
    pub errors: Vec<ValidationError>,
}

impl fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Configuration validation failed with {} error(s):",
            self.errors.len()
        )?;
        for error in &self.errors {
            writeln!(f, "  - {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigValidationError {}

/// Top-level configuration error.
#[derive(Debug, Clone)]
pub enum ConfigError {
    /// The configuration file was not found.
    FileNotFound(PathBuf),
    /// An I/O error occurred reading the file.
    IoError(String),
    /// The TOML content could not be parsed.
    ParseError(String),
    /// One or more configuration settings are invalid.
    Validation(ConfigValidationError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::FileNotFound(path) => {
                write!(f, "Configuration file not found: {}", path.display())
            }
            ConfigError::IoError(msg) => write!(f, "I/O error reading configuration: {msg}"),
            ConfigError::ParseError(msg) => write!(f, "Failed to parse configuration: {msg}"),
            ConfigError::Validation(errors) => write!(f, "{errors}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Validate the entire platform configuration, collecting all errors.
///
/// Returns an empty Vec if the configuration is valid.
pub fn validate_config(config: &PlatformConfig) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    validate_general(config, &mut errors);
    validate_llm(config, &mut errors);
    validate_security(config, &mut errors);
    validate_coding(config, &mut errors);
    validate_scheduler(config, &mut errors);
    validate_web(config, &mut errors);
    validate_serve(config, &mut errors);
    validate_monitoring(config, &mut errors);

    errors
}

fn validate_general(config: &PlatformConfig, errors: &mut Vec<ValidationError>) {
    if config.general.agent_name.trim().is_empty() {
        errors.push(ValidationError {
            setting: "general.agent_name".to_string(),
            reason: "agent name must not be empty".to_string(),
        });
    }
}

fn validate_llm(config: &PlatformConfig, errors: &mut Vec<ValidationError>) {
    if config.llm.providers.is_empty() {
        errors.push(ValidationError {
            setting: "llm.providers".to_string(),
            reason: "at least 1 LLM provider must be configured".to_string(),
        });
        return;
    }

    if config.llm.providers.len() > 10 {
        errors.push(ValidationError {
            setting: "llm.providers".to_string(),
            reason: format!(
                "at most 10 LLM providers are allowed, found {}",
                config.llm.providers.len()
            ),
        });
    }

    // Check for unique priorities
    let mut priorities = HashSet::new();
    for (i, provider) in config.llm.providers.iter().enumerate() {
        let prefix = format!("llm.providers[{i}]");

        if provider.name.trim().is_empty() {
            errors.push(ValidationError {
                setting: format!("{prefix}.name"),
                reason: "provider name must not be empty".to_string(),
            });
        }

        let is_cli_provider = matches!(
            provider.provider_type,
            ProviderType::ClaudeCode
                | ProviderType::CopilotCli
                | ProviderType::GeminiCli
                | ProviderType::CodexCli
        );

        if !is_cli_provider && provider.base_url.trim().is_empty() {
            errors.push(ValidationError {
                setting: format!("{prefix}.base_url"),
                reason: "base_url must not be empty".to_string(),
            });
        }

        if provider.model.trim().is_empty() {
            errors.push(ValidationError {
                setting: format!("{prefix}.model"),
                reason: "model must not be empty".to_string(),
            });
        }

        // API key is required for non-Ollama, non-CLI providers
        let needs_api_key = !matches!(
            provider.provider_type,
            ProviderType::Ollama
                | ProviderType::ClaudeCode
                | ProviderType::CopilotCli
                | ProviderType::GeminiCli
                | ProviderType::CodexCli
        );
        if needs_api_key && provider.api_key.is_none() {
            errors.push(ValidationError {
                setting: format!("{prefix}.api_key"),
                reason: format!(
                    "api_key is required for {} providers",
                    provider.provider_type
                ),
            });
        }

        // Timeout must be 5–120 seconds
        if provider.timeout_seconds < 5 || provider.timeout_seconds > 120 {
            errors.push(ValidationError {
                setting: format!("{prefix}.timeout_seconds"),
                reason: format!(
                    "timeout_seconds must be between 5 and 120, got {}",
                    provider.timeout_seconds
                ),
            });
        }

        // Check for duplicate priorities
        if !priorities.insert(provider.priority) {
            errors.push(ValidationError {
                setting: format!("{prefix}.priority"),
                reason: format!(
                    "duplicate priority value {}; each provider must have a unique priority",
                    provider.priority
                ),
            });
        }
    }
}

fn validate_security(config: &PlatformConfig, errors: &mut Vec<ValidationError>) {
    if config.security.session_timeout_minutes == 0 {
        errors.push(ValidationError {
            setting: "security.session_timeout_minutes".to_string(),
            reason: "session timeout must be greater than 0".to_string(),
        });
    }

    if config.security.max_failed_attempts == 0 {
        errors.push(ValidationError {
            setting: "security.max_failed_attempts".to_string(),
            reason: "max failed attempts must be greater than 0".to_string(),
        });
    }

    if config.security.lockout_minutes == 0 {
        errors.push(ValidationError {
            setting: "security.lockout_minutes".to_string(),
            reason: "lockout duration must be greater than 0".to_string(),
        });
    }

    // Resource limits
    if config.security.resource_limits.max_memory_mb == 0 {
        errors.push(ValidationError {
            setting: "security.resource_limits.max_memory_mb".to_string(),
            reason: "max memory must be greater than 0".to_string(),
        });
    }

    if config.security.resource_limits.max_cpu_percent == 0
        || config.security.resource_limits.max_cpu_percent > 100
    {
        errors.push(ValidationError {
            setting: "security.resource_limits.max_cpu_percent".to_string(),
            reason: "max CPU percent must be between 1 and 100".to_string(),
        });
    }

    if config.security.resource_limits.max_processes == 0 {
        errors.push(ValidationError {
            setting: "security.resource_limits.max_processes".to_string(),
            reason: "max processes must be greater than 0".to_string(),
        });
    }
}

fn validate_coding(config: &PlatformConfig, errors: &mut Vec<ValidationError>) {
    if let Some(ref coding) = config.coding {
        if coding.workspace_dirs.is_empty() {
            errors.push(ValidationError {
                setting: "coding.workspace_dirs".to_string(),
                reason: "at least one workspace directory must be configured when coding module is enabled".to_string(),
            });
        }

        if coding.max_file_size_mb == 0 {
            errors.push(ValidationError {
                setting: "coding.max_file_size_mb".to_string(),
                reason: "max file size must be greater than 0".to_string(),
            });
        }

        if coding.max_concurrent_shells == 0 {
            errors.push(ValidationError {
                setting: "coding.max_concurrent_shells".to_string(),
                reason: "max concurrent shells must be greater than 0".to_string(),
            });
        }

        if coding.shell_timeout_seconds == 0 {
            errors.push(ValidationError {
                setting: "coding.shell_timeout_seconds".to_string(),
                reason: "shell timeout must be greater than 0".to_string(),
            });
        }

        if coding.undo_history_size == 0 {
            errors.push(ValidationError {
                setting: "coding.undo_history_size".to_string(),
                reason: "undo history size must be greater than 0".to_string(),
            });
        }

        // Validate language server configs
        for (i, lsp) in coding.language_servers.iter().enumerate() {
            if lsp.language.trim().is_empty() {
                errors.push(ValidationError {
                    setting: format!("coding.language_servers[{i}].language"),
                    reason: "language must not be empty".to_string(),
                });
            }
            if lsp.command.trim().is_empty() {
                errors.push(ValidationError {
                    setting: format!("coding.language_servers[{i}].command"),
                    reason: "command must not be empty".to_string(),
                });
            }
        }
    }
}

fn validate_scheduler(config: &PlatformConfig, errors: &mut Vec<ValidationError>) {
    if config.scheduler.max_concurrent_tasks == 0 {
        errors.push(ValidationError {
            setting: "scheduler.max_concurrent_tasks".to_string(),
            reason: "max concurrent tasks must be greater than 0".to_string(),
        });
    }

    if config.scheduler.default_timeout_seconds == 0 {
        errors.push(ValidationError {
            setting: "scheduler.default_timeout_seconds".to_string(),
            reason: "default timeout must be greater than 0".to_string(),
        });
    }

    if config.scheduler.max_dependency_depth == 0 {
        errors.push(ValidationError {
            setting: "scheduler.max_dependency_depth".to_string(),
            reason: "max dependency depth must be greater than 0".to_string(),
        });
    }
}

fn validate_web(config: &PlatformConfig, errors: &mut Vec<ValidationError>) {
    if config.web.enabled {
        if config.web.host.trim().is_empty() {
            errors.push(ValidationError {
                setting: "web.host".to_string(),
                reason: "host must not be empty when web interface is enabled".to_string(),
            });
        }

        // If one TLS field is set, both must be set
        match (&config.web.tls_cert, &config.web.tls_key) {
            (Some(_), None) => {
                errors.push(ValidationError {
                    setting: "web.tls_key".to_string(),
                    reason: "tls_key must be provided when tls_cert is set".to_string(),
                });
            }
            (None, Some(_)) => {
                errors.push(ValidationError {
                    setting: "web.tls_cert".to_string(),
                    reason: "tls_cert must be provided when tls_key is set".to_string(),
                });
            }
            _ => {}
        }
    }
}

fn validate_serve(config: &PlatformConfig, errors: &mut Vec<ValidationError>) {
    if config.serve.host.trim().is_empty() {
        errors.push(ValidationError {
            setting: "serve.host".to_string(),
            reason: "server host must not be empty".to_string(),
        });
    }

    if config.serve.port == 0 {
        errors.push(ValidationError {
            setting: "serve.port".to_string(),
            reason: "server port must be greater than 0".to_string(),
        });
    }

    if config.serve.rate_limit_per_minute == 0 {
        errors.push(ValidationError {
            setting: "serve.rate_limit_per_minute".to_string(),
            reason: "rate limit must be greater than 0".to_string(),
        });
    }
}

fn validate_monitoring(config: &PlatformConfig, errors: &mut Vec<ValidationError>) {
    if config.monitoring.log_retention_days == 0 || config.monitoring.log_retention_days > 365 {
        errors.push(ValidationError {
            setting: "monitoring.log_retention_days".to_string(),
            reason: format!(
                "log retention must be between 1 and 365 days, got {}",
                config.monitoring.log_retention_days
            ),
        });
    }

    if config.monitoring.max_log_file_size_mb == 0 {
        errors.push(ValidationError {
            setting: "monitoring.max_log_file_size_mb".to_string(),
            reason: "max log file size must be greater than 0".to_string(),
        });
    }

    if config.monitoring.metrics_enabled && config.monitoring.metrics_port == 0 {
        errors.push(ValidationError {
            setting: "monitoring.metrics_port".to_string(),
            reason: "metrics port must be greater than 0 when metrics are enabled".to_string(),
        });
    }

    // Validate alert rules
    for (i, rule) in config.monitoring.alert_rules.iter().enumerate() {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::models::*;

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
            serve: ServeConfig::default(),
            messaging: MessagingConfig::default(),
            monitoring: MonitoringConfig::default(),
            plugins: PluginConfig::default(),
            mcp: McpConfig::default(),
            skills: SkillsConfig::default(),
        }
    }

    #[test]
    fn test_valid_config_passes_validation() {
        let config = minimal_valid_config();
        let errors = validate_config(&config);
        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
    }

    #[test]
    fn test_empty_providers_fails() {
        let mut config = minimal_valid_config();
        config.llm.providers.clear();
        let errors = validate_config(&config);
        assert!(errors.iter().any(|e| e.setting == "llm.providers"));
    }

    #[test]
    fn test_too_many_providers_fails() {
        let mut config = minimal_valid_config();
        for i in 0..11 {
            config.llm.providers.push(ProviderConfig {
                name: format!("provider-{i}"),
                provider_type: ProviderType::Ollama,
                api_key: None,
                base_url: "http://localhost:11434".to_string(),
                model: "llama3".to_string(),
                priority: i as u8,
                timeout_seconds: 30,
                max_tokens: None,
            });
        }
        let errors = validate_config(&config);
        assert!(errors
            .iter()
            .any(|e| e.setting == "llm.providers" && e.reason.contains("at most 10")));
    }

    #[test]
    fn test_timeout_out_of_range_fails() {
        let mut config = minimal_valid_config();
        config.llm.providers[0].timeout_seconds = 3;
        let errors = validate_config(&config);
        assert!(errors
            .iter()
            .any(|e| e.setting == "llm.providers[0].timeout_seconds"));
    }

    #[test]
    fn test_duplicate_priorities_fails() {
        let mut config = minimal_valid_config();
        config.llm.providers.push(ProviderConfig {
            name: "second".to_string(),
            provider_type: ProviderType::Ollama,
            api_key: None,
            base_url: "http://localhost:11434".to_string(),
            model: "llama3".to_string(),
            priority: 1, // same as first
            timeout_seconds: 30,
            max_tokens: None,
        });
        let errors = validate_config(&config);
        assert!(errors
            .iter()
            .any(|e| e.reason.contains("duplicate priority")));
    }

    #[test]
    fn test_api_key_required_for_non_ollama() {
        let mut config = minimal_valid_config();
        config.llm.providers[0].provider_type = ProviderType::Anthropic;
        config.llm.providers[0].api_key = None;
        let errors = validate_config(&config);
        assert!(errors
            .iter()
            .any(|e| e.setting == "llm.providers[0].api_key"));
    }

    #[test]
    fn test_log_retention_out_of_range() {
        let mut config = minimal_valid_config();
        config.monitoring.log_retention_days = 0;
        let errors = validate_config(&config);
        assert!(errors
            .iter()
            .any(|e| e.setting == "monitoring.log_retention_days"));

        let mut config = minimal_valid_config();
        config.monitoring.log_retention_days = 400;
        let errors = validate_config(&config);
        assert!(errors
            .iter()
            .any(|e| e.setting == "monitoring.log_retention_days"));
    }

    #[test]
    fn test_coding_requires_workspace_dirs() {
        let mut config = minimal_valid_config();
        config.coding = Some(CodingConfig {
            workspace_dirs: vec![],
            repository_dirs: vec![],
            command_allowlist: vec![],
            command_blocklist: vec![],
            max_file_size_mb: 10,
            max_concurrent_shells: 5,
            shell_timeout_seconds: 300,
            undo_history_size: 50,
            language_servers: vec![],
        });
        let errors = validate_config(&config);
        assert!(errors.iter().any(|e| e.setting == "coding.workspace_dirs"));
    }

    #[test]
    fn test_multiple_errors_collected() {
        let mut config = minimal_valid_config();
        config.llm.providers.clear();
        config.security.session_timeout_minutes = 0;
        config.monitoring.log_retention_days = 0;
        config.serve.port = 0;

        let errors = validate_config(&config);
        // Should have at least 4 errors
        assert!(
            errors.len() >= 4,
            "Expected at least 4 errors, got {}: {errors:?}",
            errors.len()
        );
    }

    #[test]
    fn test_tls_cert_without_key_fails() {
        let mut config = minimal_valid_config();
        config.web.tls_cert = Some("/path/to/cert.pem".into());
        config.web.tls_key = None;
        let errors = validate_config(&config);
        assert!(errors.iter().any(|e| e.setting == "web.tls_key"));
    }

    #[test]
    fn test_tls_key_without_cert_fails() {
        let mut config = minimal_valid_config();
        config.web.tls_cert = None;
        config.web.tls_key = Some("/path/to/key.pem".into());
        let errors = validate_config(&config);
        assert!(errors.iter().any(|e| e.setting == "web.tls_cert"));
    }
}
