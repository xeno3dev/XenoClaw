//! Integration tests for the configuration system.
//!
//! Tests TOML loading, environment variable overrides, and validation.

use common::config::{self, ConfigError, PlatformConfig};
use std::env;
use std::io::Write;
use std::sync::Mutex;
use tempfile::NamedTempFile;

/// Global mutex to serialize tests that modify environment variables.
/// Environment variables are process-global, so parallel tests can interfere.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Helper to create a temporary TOML config file with the given content.
fn write_temp_config(content: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    file.write_all(content.as_bytes())
        .expect("Failed to write config");
    file
}

/// A minimal valid TOML configuration.
const MINIMAL_VALID_TOML: &str = r#"
[general]
agent_name = "test-agent"
data_dir = "/tmp/data"
log_dir = "/tmp/logs"

[[llm.providers]]
name = "local-ollama"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = 1
timeout_seconds = 30
"#;

#[test]
fn test_load_valid_toml_config() {
    let file = write_temp_config(MINIMAL_VALID_TOML);
    let config = config::load_config(file.path()).expect("Should load valid config");

    assert_eq!(config.general.agent_name, "test-agent");
    assert_eq!(config.llm.providers.len(), 1);
    assert_eq!(config.llm.providers[0].name, "local-ollama");
    assert_eq!(config.llm.providers[0].model, "llama3");
}

#[test]
fn test_load_config_from_str() {
    let config =
        config::load_config_from_str(MINIMAL_VALID_TOML).expect("Should load valid config");

    assert_eq!(config.general.agent_name, "test-agent");
    assert_eq!(config.llm.providers.len(), 1);
}

#[test]
fn test_file_not_found_error() {
    let result = config::load_config(std::path::Path::new("/nonexistent/path/config.toml"));
    assert!(matches!(result, Err(ConfigError::FileNotFound(_))));
}

#[test]
fn test_invalid_toml_parse_error() {
    let result = config::load_config_from_str("this is not valid [[[toml");
    assert!(matches!(result, Err(ConfigError::ParseError(_))));
}

#[test]
fn test_validation_rejects_empty_providers() {
    let toml = r#"
[general]
agent_name = "test"

[llm]
"#;
    let result = config::load_config_from_str(toml);
    match result {
        Err(ConfigError::Validation(ref v)) => {
            assert!(v.errors.iter().any(|e| e.setting == "llm.providers"));
        }
        other => panic!("Expected validation error, got: {other:?}"),
    }
}

#[test]
fn test_validation_collects_all_errors() {
    // Config with multiple issues: no providers, invalid log retention, zero API port
    let toml = r#"
[general]
agent_name = "test"

[monitoring]
log_retention_days = 0

[serve]
port = 0
"#;
    let result = config::load_config_from_str(toml);
    match result {
        Err(ConfigError::Validation(ref v)) => {
            // Should have at least 3 errors: no providers, bad retention, bad port
            assert!(
                v.errors.len() >= 3,
                "Expected at least 3 errors, got {}: {:?}",
                v.errors.len(),
                v.errors
            );
        }
        other => panic!("Expected validation error, got: {other:?}"),
    }
}

#[test]
fn test_env_override_takes_precedence_over_toml() {
    let _lock = lock_env();
    // Set env var to override the agent name
    env::set_var("XENOCLAW_GENERAL__AGENT_NAME", "env-override-agent");

    let config =
        config::load_config_from_str(MINIMAL_VALID_TOML).expect("Should load valid config");

    assert_eq!(config.general.agent_name, "env-override-agent");

    // Clean up
    env::remove_var("XENOCLAW_GENERAL__AGENT_NAME");
}

#[test]
fn test_env_override_provider_api_key() {
    let _lock = lock_env();
    env::set_var("XENOCLAW_LLM__PROVIDERS__0__API_KEY", "sk-secret-from-env");

    let config =
        config::load_config_from_str(MINIMAL_VALID_TOML).expect("Should load valid config");

    assert_eq!(
        config.llm.providers[0].api_key,
        Some("sk-secret-from-env".to_string())
    );

    // Clean up
    env::remove_var("XENOCLAW_LLM__PROVIDERS__0__API_KEY");
}

#[test]
fn test_env_override_provider_model() {
    let _lock = lock_env();
    env::set_var("XENOCLAW_LLM__PROVIDERS__0__MODEL", "gpt-4o");

    let config =
        config::load_config_from_str(MINIMAL_VALID_TOML).expect("Should load valid config");

    assert_eq!(config.llm.providers[0].model, "gpt-4o");

    // Clean up
    env::remove_var("XENOCLAW_LLM__PROVIDERS__0__MODEL");
}

#[test]
fn test_env_override_security_settings() {
    let _lock = lock_env();
    env::set_var("XENOCLAW_SECURITY__SESSION_TIMEOUT_MINUTES", "60");
    env::set_var("XENOCLAW_SECURITY__MAX_FAILED_ATTEMPTS", "10");

    let config =
        config::load_config_from_str(MINIMAL_VALID_TOML).expect("Should load valid config");

    assert_eq!(config.security.session_timeout_minutes, 60);
    assert_eq!(config.security.max_failed_attempts, 10);

    // Clean up
    env::remove_var("XENOCLAW_SECURITY__SESSION_TIMEOUT_MINUTES");
    env::remove_var("XENOCLAW_SECURITY__MAX_FAILED_ATTEMPTS");
}

#[test]
fn test_env_override_monitoring_settings() {
    let _lock = lock_env();
    env::set_var("XENOCLAW_MONITORING__LOG_LEVEL", "debug");
    env::set_var("XENOCLAW_MONITORING__METRICS_PORT", "9200");

    let config =
        config::load_config_from_str(MINIMAL_VALID_TOML).expect("Should load valid config");

    assert_eq!(config.monitoring.log_level, common::config::LogLevel::Debug);
    assert_eq!(config.monitoring.metrics_port, 9200);

    // Clean up
    env::remove_var("XENOCLAW_MONITORING__LOG_LEVEL");
    env::remove_var("XENOCLAW_MONITORING__METRICS_PORT");
}

#[test]
fn test_multiple_providers_with_priorities() {
    let toml = r#"
[general]
agent_name = "multi-provider"

[[llm.providers]]
name = "anthropic"
provider_type = "anthropic"
api_key = "sk-ant-test"
base_url = "https://api.anthropic.com"
model = "claude-3-sonnet"
priority = 1
timeout_seconds = 60

[[llm.providers]]
name = "ollama-fallback"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = 2
timeout_seconds = 30
"#;

    let config = config::load_config_from_str(toml).expect("Should load valid config");
    assert_eq!(config.llm.providers.len(), 2);
    assert_eq!(config.llm.providers[0].priority, 1);
    assert_eq!(config.llm.providers[1].priority, 2);
}

#[test]
fn test_coding_config_optional() {
    // Without coding section, it should be None
    let config =
        config::load_config_from_str(MINIMAL_VALID_TOML).expect("Should load valid config");
    assert!(config.coding.is_none());
}

#[test]
fn test_coding_config_present() {
    let toml = r#"
[general]
agent_name = "coding-agent"

[[llm.providers]]
name = "ollama"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "codellama"
priority = 1

[coding]
workspace_dirs = ["/home/user/projects"]
repository_dirs = ["/home/user/projects"]
command_allowlist = ["cargo", "npm", "git"]
max_file_size_mb = 10
max_concurrent_shells = 5
shell_timeout_seconds = 300
undo_history_size = 50
"#;

    let config = config::load_config_from_str(toml).expect("Should load valid config");
    assert!(config.coding.is_some());
    let coding = config.coding.unwrap();
    assert_eq!(coding.workspace_dirs.len(), 1);
    assert_eq!(coding.command_allowlist.len(), 3);
}

#[test]
fn test_provider_timeout_validation_bounds() {
    // Timeout too low (< 5)
    let toml = r#"
[general]
agent_name = "test"

[[llm.providers]]
name = "fast"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = 1
timeout_seconds = 2
"#;
    let result = config::load_config_from_str(toml);
    assert!(matches!(result, Err(ConfigError::Validation(_))));

    // Timeout too high (> 120)
    let toml = r#"
[general]
agent_name = "test"

[[llm.providers]]
name = "slow"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = 1
timeout_seconds = 200
"#;
    let result = config::load_config_from_str(toml);
    assert!(matches!(result, Err(ConfigError::Validation(_))));
}

#[test]
fn test_defaults_applied_for_missing_sections() {
    let _lock = lock_env();
    // Only provide the minimum required (general + one provider)
    let config =
        config::load_config_from_str(MINIMAL_VALID_TOML).expect("Should load valid config");

    // Check defaults are applied
    assert_eq!(config.security.session_timeout_minutes, 30);
    assert_eq!(config.security.max_failed_attempts, 5);
    assert_eq!(config.security.lockout_minutes, 15);
    assert_eq!(config.scheduler.max_concurrent_tasks, 10);
    assert_eq!(config.scheduler.default_timeout_seconds, 300);
    assert_eq!(config.monitoring.log_retention_days, 30);
    assert!(config.monitoring.metrics_enabled);
    assert_eq!(config.serve.rate_limit_per_minute, 100);
    assert!(config.web.enabled);
}

#[test]
fn test_refuse_to_start_with_invalid_config() {
    // This simulates the "refuse to start" requirement — the load function
    // returns an error that the caller must handle by not starting.
    let toml = r#"
[general]
agent_name = ""
"#;
    let result = config::load_config_from_str(toml);
    assert!(result.is_err(), "Should refuse invalid config");

    if let Err(ConfigError::Validation(ref v)) = result {
        // Verify the error identifies the setting by name
        assert!(v
            .errors
            .iter()
            .any(|e| e.setting.contains("agent_name") || e.setting.contains("llm.providers")));
    }
}
