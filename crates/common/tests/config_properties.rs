//! Property-based tests for the configuration system.
//!
//! **Validates: Requirements 12.3, 12.4**
//!
//! Property 22: Configuration Precedence (Env over TOML)
//! Property 23: Configuration Validation Completeness
//!
//! NOTE: Tests that use environment variables must run serially (single-threaded)
//! because `env::set_var` is process-global and not thread-safe.

use common::config::{self, ConfigError, ValidationError};
use proptest::prelude::*;
use std::env;
use std::sync::Mutex;

/// Global mutex to serialize env-var-dependent tests.
/// This prevents race conditions when multiple proptest cases run concurrently.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// A minimal valid TOML config that passes validation.
/// Used as a base for env override tests.
const VALID_BASE_TOML: &str = r#"
[general]
agent_name = "base-agent"
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

// ============================================================================
// Strategies for generating valid config values
// ============================================================================

/// Generate a non-empty string suitable for agent names.
fn non_empty_string_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_-]{0,30}".prop_map(|s| s)
}

/// Generate a valid port number (1-65535).
fn valid_port_strategy() -> impl Strategy<Value = u16> {
    1..=65535u16
}

/// Generate a valid timeout value (5-120).
fn valid_timeout_strategy() -> impl Strategy<Value = u32> {
    5..=120u32
}

/// Generate a valid session timeout (> 0).
fn valid_session_timeout_strategy() -> impl Strategy<Value = u32> {
    1..=1440u32
}

/// Generate a valid max_concurrent_tasks (> 0).
fn valid_max_concurrent_tasks_strategy() -> impl Strategy<Value = u8> {
    1..=255u8
}

/// Generate a valid rate limit (> 0).
fn valid_rate_limit_strategy() -> impl Strategy<Value = u32> {
    1..=10000u32
}

/// Generate a valid log retention days (1-365).
fn valid_log_retention_strategy() -> impl Strategy<Value = u16> {
    1..=365u16
}

// ============================================================================
// Property 22: Configuration Precedence (Env over TOML)
//
// For any valid TOML configuration and any environment variable override,
// the resulting loaded configuration SHALL reflect the environment variable
// value rather than the TOML value for the overridden setting.
//
// **Validates: Requirements 12.3**
// ============================================================================

/// Helper to clear all XENOCLAW_ env vars that might interfere with tests.
fn clear_all_xenoclaw_env_vars() {
    let vars_to_clear = [
        "XENOCLAW_GENERAL__AGENT_NAME",
        "XENOCLAW_GENERAL__DATA_DIR",
        "XENOCLAW_GENERAL__LOG_DIR",
        "XENOCLAW_WEB__PORT",
        "XENOCLAW_WEB__HOST",
        "XENOCLAW_WEB__ENABLED",
        "XENOCLAW_API__PORT",
        "XENOCLAW_API__HOST",
        "XENOCLAW_API__RATE_LIMIT_PER_MINUTE",
        "XENOCLAW_SECURITY__SESSION_TIMEOUT_MINUTES",
        "XENOCLAW_SECURITY__MAX_FAILED_ATTEMPTS",
        "XENOCLAW_SECURITY__LOCKOUT_MINUTES",
        "XENOCLAW_SECURITY__RESOURCE_LIMITS__MAX_MEMORY_MB",
        "XENOCLAW_SECURITY__RESOURCE_LIMITS__MAX_CPU_PERCENT",
        "XENOCLAW_SECURITY__RESOURCE_LIMITS__MAX_PROCESSES",
        "XENOCLAW_MONITORING__LOG_LEVEL",
        "XENOCLAW_MONITORING__LOG_RETENTION_DAYS",
        "XENOCLAW_MONITORING__MAX_LOG_FILE_SIZE_MB",
        "XENOCLAW_MONITORING__METRICS_ENABLED",
        "XENOCLAW_MONITORING__METRICS_PORT",
        "XENOCLAW_SCHEDULER__MAX_CONCURRENT_TASKS",
        "XENOCLAW_SCHEDULER__DEFAULT_TIMEOUT_SECONDS",
        "XENOCLAW_LLM__PROVIDERS__0__NAME",
        "XENOCLAW_LLM__PROVIDERS__0__MODEL",
        "XENOCLAW_LLM__PROVIDERS__0__BASE_URL",
        "XENOCLAW_LLM__PROVIDERS__0__API_KEY",
        "XENOCLAW_LLM__PROVIDERS__0__TIMEOUT_SECONDS",
        "XENOCLAW_LLM__PROVIDERS__0__PRIORITY",
        "XENOCLAW_PLUGINS__ENABLED",
        "XENOCLAW_PLUGINS__DIRECTORY",
    ];
    for var in vars_to_clear {
        env::remove_var(var);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// **Validates: Requirements 12.3**
    ///
    /// Property 22: For any valid agent name set via env var, the loaded config
    /// SHALL use the env var value instead of the TOML value.
    #[test]
    fn prop_env_overrides_toml_agent_name(
        env_name in non_empty_string_strategy()
    ) {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_xenoclaw_env_vars();

        env::set_var("XENOCLAW_GENERAL__AGENT_NAME", &env_name);
        let result = config::load_config_from_str(VALID_BASE_TOML);
        env::remove_var("XENOCLAW_GENERAL__AGENT_NAME");

        let config = result.expect("Config should load successfully");
        prop_assert_eq!(config.general.agent_name, env_name);
    }

    /// **Validates: Requirements 12.3**
    ///
    /// Property 22: For any valid port set via env var, the loaded config
    /// SHALL use the env var value for the web port.
    #[test]
    fn prop_env_overrides_toml_web_port(
        env_port in valid_port_strategy()
    ) {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_xenoclaw_env_vars();

        env::set_var("XENOCLAW_WEB__PORT", env_port.to_string());
        let result = config::load_config_from_str(VALID_BASE_TOML);
        env::remove_var("XENOCLAW_WEB__PORT");

        let config = result.expect("Config should load successfully");
        prop_assert_eq!(config.web.port, env_port);
    }

    /// **Validates: Requirements 12.3**
    ///
    /// Property 22: For any valid port set via env var, the loaded config
    /// SHALL use the env var value for the API port.
    #[test]
    fn prop_env_overrides_toml_api_port(
        env_port in valid_port_strategy()
    ) {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_xenoclaw_env_vars();

        env::set_var("XENOCLAW_API__PORT", env_port.to_string());
        let result = config::load_config_from_str(VALID_BASE_TOML);
        env::remove_var("XENOCLAW_API__PORT");

        let config = result.expect("Config should load successfully");
        prop_assert_eq!(config.api.port, env_port);
    }

    /// **Validates: Requirements 12.3**
    ///
    /// Property 22: For any valid timeout set via env var, the loaded config
    /// SHALL use the env var value for the provider timeout.
    #[test]
    fn prop_env_overrides_toml_provider_timeout(
        env_timeout in valid_timeout_strategy()
    ) {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_xenoclaw_env_vars();

        env::set_var("XENOCLAW_LLM__PROVIDERS__0__TIMEOUT_SECONDS", env_timeout.to_string());
        let result = config::load_config_from_str(VALID_BASE_TOML);
        env::remove_var("XENOCLAW_LLM__PROVIDERS__0__TIMEOUT_SECONDS");

        let config = result.expect("Config should load successfully");
        prop_assert_eq!(config.llm.providers[0].timeout_seconds, env_timeout);
    }

    /// **Validates: Requirements 12.3**
    ///
    /// Property 22: For any valid session timeout set via env var, the loaded config
    /// SHALL use the env var value for the security session timeout.
    #[test]
    fn prop_env_overrides_toml_session_timeout(
        env_timeout in valid_session_timeout_strategy()
    ) {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_xenoclaw_env_vars();

        env::set_var("XENOCLAW_SECURITY__SESSION_TIMEOUT_MINUTES", env_timeout.to_string());
        let result = config::load_config_from_str(VALID_BASE_TOML);
        env::remove_var("XENOCLAW_SECURITY__SESSION_TIMEOUT_MINUTES");

        let config = result.expect("Config should load successfully");
        prop_assert_eq!(config.security.session_timeout_minutes, env_timeout);
    }

    /// **Validates: Requirements 12.3**
    ///
    /// Property 22: For any valid rate limit set via env var, the loaded config
    /// SHALL use the env var value for the API rate limit.
    #[test]
    fn prop_env_overrides_toml_rate_limit(
        env_rate in valid_rate_limit_strategy()
    ) {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_xenoclaw_env_vars();

        env::set_var("XENOCLAW_API__RATE_LIMIT_PER_MINUTE", env_rate.to_string());
        let result = config::load_config_from_str(VALID_BASE_TOML);
        env::remove_var("XENOCLAW_API__RATE_LIMIT_PER_MINUTE");

        let config = result.expect("Config should load successfully");
        prop_assert_eq!(config.api.rate_limit_per_minute, env_rate);
    }

    /// **Validates: Requirements 12.3**
    ///
    /// Property 22: For any valid max concurrent tasks set via env var, the loaded config
    /// SHALL use the env var value for the scheduler setting.
    #[test]
    fn prop_env_overrides_toml_max_concurrent_tasks(
        env_tasks in valid_max_concurrent_tasks_strategy()
    ) {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_xenoclaw_env_vars();

        env::set_var("XENOCLAW_SCHEDULER__MAX_CONCURRENT_TASKS", env_tasks.to_string());
        let result = config::load_config_from_str(VALID_BASE_TOML);
        env::remove_var("XENOCLAW_SCHEDULER__MAX_CONCURRENT_TASKS");

        let config = result.expect("Config should load successfully");
        prop_assert_eq!(config.scheduler.max_concurrent_tasks, env_tasks);
    }

    /// **Validates: Requirements 12.3**
    ///
    /// Property 22: For any valid log retention days set via env var, the loaded config
    /// SHALL use the env var value for the monitoring setting.
    #[test]
    fn prop_env_overrides_toml_log_retention(
        env_days in valid_log_retention_strategy()
    ) {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_xenoclaw_env_vars();

        env::set_var("XENOCLAW_MONITORING__LOG_RETENTION_DAYS", env_days.to_string());
        let result = config::load_config_from_str(VALID_BASE_TOML);
        env::remove_var("XENOCLAW_MONITORING__LOG_RETENTION_DAYS");

        let config = result.expect("Config should load successfully");
        prop_assert_eq!(config.monitoring.log_retention_days, env_days);
    }

    /// **Validates: Requirements 12.3**
    ///
    /// Property 22: For any valid model name set via env var, the loaded config
    /// SHALL use the env var value for the provider model.
    #[test]
    fn prop_env_overrides_toml_provider_model(
        env_model in non_empty_string_strategy()
    ) {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_all_xenoclaw_env_vars();

        env::set_var("XENOCLAW_LLM__PROVIDERS__0__MODEL", &env_model);
        let result = config::load_config_from_str(VALID_BASE_TOML);
        env::remove_var("XENOCLAW_LLM__PROVIDERS__0__MODEL");

        let config = result.expect("Config should load successfully");
        prop_assert_eq!(&config.llm.providers[0].model, &env_model);
    }
}

// ============================================================================
// Property 23: Configuration Validation Completeness
//
// For any configuration with N invalid settings, the validation SHALL report
// exactly N errors, each identifying the invalid setting by name with a reason.
//
// **Validates: Requirements 12.4**
// ============================================================================

/// Helper: Load config from TOML string and extract validation errors.
/// Returns None if the config loaded successfully (no errors).
fn get_validation_errors(toml_content: &str) -> Option<Vec<ValidationError>> {
    let _lock = ENV_MUTEX.lock().unwrap();
    clear_all_xenoclaw_env_vars();

    match config::load_config_from_str(toml_content) {
        Err(ConfigError::Validation(v)) => Some(v.errors),
        _ => None,
    }
}

/// Represents a single invalid setting we can inject into a config.
#[derive(Debug, Clone)]
enum InvalidSettingKind {
    /// Invalid security setting (goes in [security] section).
    Security { toml_fragment: String, expected_setting: String },
    /// Invalid monitoring setting (goes in [monitoring] section).
    Monitoring { toml_fragment: String, expected_setting: String },
}

impl InvalidSettingKind {
    fn expected_setting(&self) -> &str {
        match self {
            InvalidSettingKind::Security { expected_setting, .. } => expected_setting,
            InvalidSettingKind::Monitoring { expected_setting, .. } => expected_setting,
        }
    }
}

/// Generate a subset of invalid settings to inject.
/// Each invalid setting is independent and produces exactly one validation error.
/// We carefully select settings that don't interact with each other.
fn invalid_settings_subset_strategy() -> impl Strategy<Value = Vec<InvalidSettingKind>> {
    // Define all possible independent invalid settings we can inject.
    // Each one produces exactly one additional validation error.
    // IMPORTANT: We only include settings from sections where we control the
    // entire section content, so no defaults interfere.
    let all_invalid_settings = vec![
        InvalidSettingKind::Security {
            toml_fragment: "max_failed_attempts = 0".to_string(),
            expected_setting: "security.max_failed_attempts".to_string(),
        },
        InvalidSettingKind::Security {
            toml_fragment: "lockout_minutes = 0".to_string(),
            expected_setting: "security.lockout_minutes".to_string(),
        },
        InvalidSettingKind::Monitoring {
            toml_fragment: "log_retention_days = 0".to_string(),
            expected_setting: "monitoring.log_retention_days".to_string(),
        },
        InvalidSettingKind::Monitoring {
            toml_fragment: "max_log_file_size_mb = 0".to_string(),
            expected_setting: "monitoring.max_log_file_size_mb".to_string(),
        },
    ];

    // Generate a boolean mask to select which invalid settings to include.
    // At least one must be selected.
    proptest::collection::vec(proptest::bool::ANY, all_invalid_settings.len()).prop_filter_map(
        "at least one invalid setting",
        move |mask| {
            let selected: Vec<InvalidSettingKind> = mask
                .iter()
                .zip(all_invalid_settings.iter())
                .filter_map(|(include, setting)| {
                    if *include {
                        Some(setting.clone())
                    } else {
                        None
                    }
                })
                .collect();

            if selected.is_empty() {
                None
            } else {
                Some(selected)
            }
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// **Validates: Requirements 12.4**
    ///
    /// Property 23: For any configuration with N invalid settings, the validation
    /// SHALL report exactly N errors, each identifying the invalid setting by name
    /// with a reason.
    #[test]
    fn prop_validation_reports_exactly_n_errors_for_n_invalid_settings(
        invalid_settings in invalid_settings_subset_strategy()
    ) {
        let n = invalid_settings.len();

        // Build a TOML config that is valid except for the selected invalid settings.
        // We construct complete sections so that defaults don't add extra valid values
        // that mask our invalid ones.
        let mut security_fragments = Vec::new();
        let mut monitoring_fragments = Vec::new();

        for setting in &invalid_settings {
            match setting {
                InvalidSettingKind::Security { toml_fragment, .. } => {
                    security_fragments.push(toml_fragment.clone());
                }
                InvalidSettingKind::Monitoring { toml_fragment, .. } => {
                    monitoring_fragments.push(toml_fragment.clone());
                }
            }
        }

        // Build security section: include valid defaults for fields we're NOT testing
        let security_section = {
            let mut lines = Vec::new();
            lines.push("[security]".to_string());
            // Only set valid defaults for fields NOT being tested
            if !security_fragments.iter().any(|f| f.contains("session_timeout_minutes")) {
                lines.push("session_timeout_minutes = 30".to_string());
            }
            if !security_fragments.iter().any(|f| f.contains("max_failed_attempts")) {
                lines.push("max_failed_attempts = 5".to_string());
            }
            if !security_fragments.iter().any(|f| f.contains("lockout_minutes")) {
                lines.push("lockout_minutes = 15".to_string());
            }
            // Add the invalid fragments
            lines.extend(security_fragments);
            lines.join("\n")
        };

        // Build monitoring section: include valid defaults for fields we're NOT testing
        let monitoring_section = {
            let mut lines = Vec::new();
            lines.push("[monitoring]".to_string());
            if !monitoring_fragments.iter().any(|f| f.contains("log_retention_days")) {
                lines.push("log_retention_days = 30".to_string());
            }
            if !monitoring_fragments.iter().any(|f| f.contains("max_log_file_size_mb")) {
                lines.push("max_log_file_size_mb = 100".to_string());
            }
            // Add the invalid fragments
            lines.extend(monitoring_fragments);
            lines.join("\n")
        };

        let toml_content = format!(
            r#"
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

{security_section}

{monitoring_section}
"#
        );

        let errors = get_validation_errors(&toml_content);
        let errors = errors.expect("Config with invalid settings should produce validation errors");

        // Verify we get exactly N errors
        prop_assert_eq!(
            errors.len(),
            n,
            "Expected exactly {} errors for {} invalid settings, got {}: {:?}",
            n,
            n,
            errors.len(),
            errors
        );

        // Verify each expected setting is identified in the errors
        for setting in &invalid_settings {
            prop_assert!(
                errors.iter().any(|e| e.setting == setting.expected_setting()),
                "Expected error for setting '{}' not found in errors: {:?}",
                setting.expected_setting(),
                errors
            );
        }

        // Verify each error has a non-empty reason
        for error in &errors {
            prop_assert!(
                !error.reason.is_empty(),
                "Error for setting '{}' has empty reason",
                error.setting
            );
            prop_assert!(
                !error.setting.is_empty(),
                "Error has empty setting name"
            );
        }
    }
}

// ============================================================================
// Additional Property 23 test: varying number of invalid provider settings
// ============================================================================

/// Strategy for generating a number of providers with invalid timeouts (1-5).
fn invalid_provider_count_strategy() -> impl Strategy<Value = u8> {
    1..=5u8
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// **Validates: Requirements 12.4**
    ///
    /// Property 23: For N providers with invalid timeout_seconds, validation
    /// SHALL report exactly N timeout errors, one per provider.
    #[test]
    fn prop_validation_reports_error_per_invalid_provider_timeout(
        num_invalid in invalid_provider_count_strategy()
    ) {
        // Build TOML with N providers, each having an invalid timeout (< 5).
        let mut providers_toml = String::new();
        for i in 0..num_invalid {
            providers_toml.push_str(&format!(
                r#"
[[llm.providers]]
name = "provider-{i}"
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = {priority}
timeout_seconds = 2
"#,
                i = i,
                priority = i + 1
            ));
        }

        let toml_content = format!(
            r#"
[general]
agent_name = "test-agent"
data_dir = "/tmp/data"
log_dir = "/tmp/logs"

{providers_toml}
"#
        );

        let errors = get_validation_errors(&toml_content);
        let errors = errors.expect("Config with invalid timeouts should produce validation errors");

        // Count timeout-specific errors
        let timeout_errors: Vec<&ValidationError> = errors
            .iter()
            .filter(|e| e.setting.contains("timeout_seconds"))
            .collect();

        prop_assert_eq!(
            timeout_errors.len(),
            num_invalid as usize,
            "Expected {} timeout errors, got {}: {:?}",
            num_invalid,
            timeout_errors.len(),
            timeout_errors
        );

        // Each error should identify the provider index
        for i in 0..num_invalid {
            let expected_setting = format!("llm.providers[{}].timeout_seconds", i);
            prop_assert!(
                timeout_errors.iter().any(|e| e.setting == expected_setting),
                "Expected error for '{}' not found in: {:?}",
                expected_setting,
                timeout_errors
            );
        }
    }

    /// **Validates: Requirements 12.4**
    ///
    /// Property 23: For N providers with empty names, validation SHALL report
    /// exactly N name errors, one per provider.
    #[test]
    fn prop_validation_reports_error_per_empty_provider_name(
        num_invalid in invalid_provider_count_strategy()
    ) {
        // Build TOML with N providers, each having an empty name.
        let mut providers_toml = String::new();
        for i in 0..num_invalid {
            providers_toml.push_str(&format!(
                r#"
[[llm.providers]]
name = ""
provider_type = "ollama"
base_url = "http://localhost:11434"
model = "llama3"
priority = {priority}
timeout_seconds = 30
"#,
                priority = i + 1
            ));
        }

        let toml_content = format!(
            r#"
[general]
agent_name = "test-agent"
data_dir = "/tmp/data"
log_dir = "/tmp/logs"

{providers_toml}
"#
        );

        let errors = get_validation_errors(&toml_content);
        let errors = errors.expect("Config with empty names should produce validation errors");

        // Count name-specific errors
        let name_errors: Vec<&ValidationError> = errors
            .iter()
            .filter(|e| e.setting.contains(".name") && e.reason.contains("name"))
            .collect();

        prop_assert_eq!(
            name_errors.len(),
            num_invalid as usize,
            "Expected {} name errors, got {}: {:?}",
            num_invalid,
            name_errors.len(),
            name_errors
        );
    }
}
