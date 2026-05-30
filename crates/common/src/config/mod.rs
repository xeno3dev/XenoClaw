//! Configuration system for the VPS AI Agent Platform.
//!
//! Supports TOML file loading with environment variable overrides.
//! Environment variables use the `XENOCLAW_` prefix with `__` for nesting.
//! For example, `XENOCLAW_LLM__PROVIDERS__0__NAME` overrides `llm.providers[0].name`.
//!
//! The configuration system validates all settings and reports ALL errors
//! (does not stop at the first invalid setting).

mod models;
pub mod reload;
pub mod signal;
mod validation;

pub use models::*;
pub use reload::{apply_reload, reload_config, reload_config_from_str, ReloadableConfig};
pub use signal::{spawn_reload_handler, trigger_reload};
pub use validation::{validate_config, ConfigError, ConfigValidationError, ValidationError};

use std::path::Path;

/// Load configuration from a TOML file, applying environment variable overrides.
///
/// Environment variables with the `XENOCLAW_` prefix override TOML values.
/// Nesting is indicated by `__` (double underscore).
///
/// # Errors
///
/// Returns `ConfigError::FileNotFound` if the file doesn't exist.
/// Returns `ConfigError::ParseError` if the TOML is malformed.
/// Returns `ConfigError::Validation` if any settings are invalid.
pub fn load_config(path: &Path) -> Result<PlatformConfig, ConfigError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ConfigError::FileNotFound(path.to_path_buf())
        } else {
            ConfigError::IoError(e.to_string())
        }
    })?;

    load_config_from_str(&content)
}

/// Load configuration from a TOML string, applying environment variable overrides.
///
/// This is useful for testing or when the config content is already in memory.
pub fn load_config_from_str(toml_content: &str) -> Result<PlatformConfig, ConfigError> {
    let mut config: PlatformConfig =
        toml::from_str(toml_content).map_err(|e| ConfigError::ParseError(e.to_string()))?;

    apply_env_overrides(&mut config);

    let errors = validation::validate_config(&config);
    if !errors.is_empty() {
        return Err(ConfigError::Validation(ConfigValidationError { errors }));
    }

    Ok(config)
}

/// Apply environment variable overrides to the configuration.
///
/// Convention: `XENOCLAW_<SECTION>__<FIELD>` maps to `section.field` in TOML.
/// Arrays use numeric indices: `XENOCLAW_LLM__PROVIDERS__0__NAME`.
fn apply_env_overrides(config: &mut PlatformConfig) {
    // General section
    if let Ok(val) = std::env::var("XENOCLAW_GENERAL__AGENT_NAME") {
        config.general.agent_name = val;
    }
    if let Ok(val) = std::env::var("XENOCLAW_GENERAL__DATA_DIR") {
        config.general.data_dir = val.into();
    }
    if let Ok(val) = std::env::var("XENOCLAW_GENERAL__LOG_DIR") {
        config.general.log_dir = val.into();
    }

    // LLM providers
    apply_llm_env_overrides(config);

    // Security section
    apply_security_env_overrides(config);

    // Coding section
    apply_coding_env_overrides(config);

    // Monitoring section
    apply_monitoring_env_overrides(config);

    // Scheduler section
    if let Ok(val) = std::env::var("XENOCLAW_SCHEDULER__MAX_CONCURRENT_TASKS") {
        if let Ok(v) = val.parse() {
            config.scheduler.max_concurrent_tasks = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_SCHEDULER__DEFAULT_TIMEOUT_SECONDS") {
        if let Ok(v) = val.parse() {
            config.scheduler.default_timeout_seconds = v;
        }
    }

    // Web section
    if let Ok(val) = std::env::var("XENOCLAW_WEB__ENABLED") {
        if let Ok(v) = val.parse() {
            config.web.enabled = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_WEB__HOST") {
        config.web.host = val;
    }

    // API section
    if let Ok(val) = std::env::var("XENOCLAW_API__HOST") {
        config.api.host = val;
    }
    if let Ok(val) = std::env::var("XENOCLAW_API__PORT") {
        if let Ok(v) = val.parse() {
            config.api.port = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_API__RATE_LIMIT_PER_MINUTE") {
        if let Ok(v) = val.parse() {
            config.api.rate_limit_per_minute = v;
        }
    }

    // Plugins section
    if let Ok(val) = std::env::var("XENOCLAW_PLUGINS__ENABLED") {
        if let Ok(v) = val.parse() {
            config.plugins.enabled = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_PLUGINS__DIRECTORY") {
        config.plugins.directory = val.into();
    }
}

fn apply_llm_env_overrides(config: &mut PlatformConfig) {
    // Override existing providers by index
    for (i, provider) in config.llm.providers.iter_mut().enumerate() {
        if let Ok(val) = std::env::var(format!("XENOCLAW_LLM__PROVIDERS__{i}__NAME")) {
            provider.name = val;
        }
        if let Ok(val) = std::env::var(format!("XENOCLAW_LLM__PROVIDERS__{i}__PROVIDER_TYPE")) {
            if let Ok(pt) = val.parse() {
                provider.provider_type = pt;
            }
        }
        if let Ok(val) = std::env::var(format!("XENOCLAW_LLM__PROVIDERS__{i}__API_KEY")) {
            provider.api_key = Some(val);
        }
        if let Ok(val) = std::env::var(format!("XENOCLAW_LLM__PROVIDERS__{i}__BASE_URL")) {
            provider.base_url = val;
        }
        if let Ok(val) = std::env::var(format!("XENOCLAW_LLM__PROVIDERS__{i}__MODEL")) {
            provider.model = val;
        }
        if let Ok(val) = std::env::var(format!("XENOCLAW_LLM__PROVIDERS__{i}__PRIORITY")) {
            if let Ok(v) = val.parse() {
                provider.priority = v;
            }
        }
        if let Ok(val) = std::env::var(format!("XENOCLAW_LLM__PROVIDERS__{i}__TIMEOUT_SECONDS")) {
            if let Ok(v) = val.parse() {
                provider.timeout_seconds = v;
            }
        }
        if let Ok(val) = std::env::var(format!("XENOCLAW_LLM__PROVIDERS__{i}__MAX_TOKENS")) {
            if let Ok(v) = val.parse::<u32>() {
                provider.max_tokens = Some(v);
            }
        }
    }
}

fn apply_security_env_overrides(config: &mut PlatformConfig) {
    if let Ok(val) = std::env::var("XENOCLAW_SECURITY__SESSION_TIMEOUT_MINUTES") {
        if let Ok(v) = val.parse() {
            config.security.session_timeout_minutes = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_SECURITY__MAX_FAILED_ATTEMPTS") {
        if let Ok(v) = val.parse() {
            config.security.max_failed_attempts = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_SECURITY__LOCKOUT_MINUTES") {
        if let Ok(v) = val.parse() {
            config.security.lockout_minutes = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_SECURITY__RESOURCE_LIMITS__MAX_MEMORY_MB") {
        if let Ok(v) = val.parse() {
            config.security.resource_limits.max_memory_mb = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_SECURITY__RESOURCE_LIMITS__MAX_CPU_PERCENT") {
        if let Ok(v) = val.parse() {
            config.security.resource_limits.max_cpu_percent = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_SECURITY__RESOURCE_LIMITS__MAX_PROCESSES") {
        if let Ok(v) = val.parse() {
            config.security.resource_limits.max_processes = v;
        }
    }
}

fn apply_coding_env_overrides(config: &mut PlatformConfig) {
    if let Some(ref mut coding) = config.coding {
        if let Ok(val) = std::env::var("XENOCLAW_CODING__MAX_FILE_SIZE_MB") {
            if let Ok(v) = val.parse() {
                coding.max_file_size_mb = v;
            }
        }
        if let Ok(val) = std::env::var("XENOCLAW_CODING__MAX_CONCURRENT_SHELLS") {
            if let Ok(v) = val.parse() {
                coding.max_concurrent_shells = v;
            }
        }
        if let Ok(val) = std::env::var("XENOCLAW_CODING__SHELL_TIMEOUT_SECONDS") {
            if let Ok(v) = val.parse() {
                coding.shell_timeout_seconds = v;
            }
        }
        if let Ok(val) = std::env::var("XENOCLAW_CODING__UNDO_HISTORY_SIZE") {
            if let Ok(v) = val.parse() {
                coding.undo_history_size = v;
            }
        }
    }
}

fn apply_monitoring_env_overrides(config: &mut PlatformConfig) {
    if let Ok(val) = std::env::var("XENOCLAW_MONITORING__LOG_LEVEL") {
        if let Ok(v) = val.parse() {
            config.monitoring.log_level = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_MONITORING__LOG_RETENTION_DAYS") {
        if let Ok(v) = val.parse() {
            config.monitoring.log_retention_days = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_MONITORING__MAX_LOG_FILE_SIZE_MB") {
        if let Ok(v) = val.parse() {
            config.monitoring.max_log_file_size_mb = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_MONITORING__METRICS_ENABLED") {
        if let Ok(v) = val.parse() {
            config.monitoring.metrics_enabled = v;
        }
    }
    if let Ok(val) = std::env::var("XENOCLAW_MONITORING__METRICS_PORT") {
        if let Ok(v) = val.parse() {
            config.monitoring.metrics_port = v;
        }
    }
}
