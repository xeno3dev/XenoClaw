//! Platform-wide error type hierarchy for the VPS AI Agent Platform.
//!
//! Errors are classified by domain and severity to enable appropriate
//! handling strategies (retry, fail-fast, alert, restart).

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::models::{AccessType, Role};

// =============================================================================
// Top-Level Platform Error
// =============================================================================

/// The top-level error type encompassing all platform error domains.
#[derive(Debug, Error)]
pub enum PlatformError {
    /// LLM-related errors (provider timeouts, failures, invalid responses).
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),

    /// Security violations (auth failures, sandbox breaches, rate limits).
    #[error("Security error: {0}")]
    Security(#[from] SecurityError),

    /// Storage errors (unavailability, capacity, corruption).
    #[error("Storage error: {0}")]
    Storage(#[from] StoreError),

    /// Task execution errors (scheduling, dependency, timeout).
    #[error("Task error: {0}")]
    Task(#[from] TaskError),

    /// Plugin errors (isolated, never crash core).
    #[error("Plugin error: {0}")]
    Plugin(#[from] PluginError),

    /// Configuration errors (invalid settings, missing values).
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),
}

// =============================================================================
// LLM Errors
// =============================================================================

/// Errors related to LLM provider communication and routing.
#[derive(Debug, Error)]
pub enum LlmError {
    /// A provider timed out before responding.
    #[error("Provider '{provider}' timed out after {elapsed_ms}ms")]
    Timeout { provider: String, elapsed_ms: u64 },

    /// All configured providers failed to respond.
    #[error("All providers failed ({} attempts)", attempts.len())]
    AllProvidersFailed { attempts: Vec<ProviderAttempt> },

    /// A provider returned an invalid or unparseable response.
    #[error("Invalid response from provider '{provider}': {reason}")]
    InvalidResponse { provider: String, reason: String },

    /// A provider rate-limited the request.
    #[error("Provider '{provider}' rate limited, retry after {retry_after:?}")]
    RateLimited {
        provider: String,
        retry_after: Duration,
    },
}

/// Record of a single provider attempt during failover routing.
#[derive(Debug, Clone)]
pub struct ProviderAttempt {
    pub provider: String,
    pub error: String,
    pub elapsed_ms: u64,
}

// =============================================================================
// Security Errors
// =============================================================================

/// Errors related to authentication, authorization, and sandboxing.
#[derive(Debug, Error)]
pub enum SecurityError {
    /// No credentials were provided.
    #[error("Authentication required")]
    AuthenticationRequired,

    /// The provided credentials are invalid.
    #[error("Invalid credentials")]
    InvalidCredentials,

    /// The source IP has been blocked due to repeated failures.
    #[error("IP blocked until {until}")]
    IpBlocked { until: DateTime<Utc> },

    /// The authenticated user lacks the required role/permissions.
    #[error("Insufficient permissions, required role: {required_role:?}")]
    InsufficientPermissions { required_role: Role },

    /// A filesystem access attempt violated sandbox boundaries.
    #[error("Sandbox violation: {access:?} access denied for path '{}'", path.display())]
    SandboxViolation { path: PathBuf, access: AccessType },

    /// A network connection attempt was blocked by the allowlist.
    #[error("Network access blocked: {host}:{port}")]
    NetworkBlocked { host: String, port: u16 },

    /// The API key has exceeded its configured rate limit.
    #[error("Rate limit exceeded, retry after {retry_after:?}")]
    RateLimitExceeded { retry_after: Duration },

    /// The session has expired due to inactivity.
    #[error("Session expired")]
    SessionExpired,
}

// =============================================================================
// Store Errors
// =============================================================================

/// Errors related to the memory/persistence store.
#[derive(Debug, Error)]
pub enum StoreError {
    /// The store is temporarily unavailable.
    #[error("Store unavailable")]
    Unavailable,

    /// The store has reached its configured capacity limit.
    #[error("Store capacity full")]
    CapacityFull,

    /// The requested entry was not found.
    #[error("Entry not found: {id}")]
    EntryNotFound { id: String },

    /// The content exceeds the maximum allowed size.
    #[error("Content too large: {actual} bytes exceeds maximum of {max} bytes")]
    ContentTooLarge { max: usize, actual: usize },

    /// Stored data is corrupted or unreadable.
    #[error("Corrupted data: {details}")]
    CorruptedData { details: String },
}

// =============================================================================
// Task Errors
// =============================================================================

/// Errors related to task scheduling and execution.
#[derive(Debug, Error)]
pub enum TaskError {
    /// The task was not found.
    #[error("Task not found: {id}")]
    NotFound { id: String },

    /// The task definition contains a dependency cycle.
    #[error("Dependency cycle detected involving tasks: {}", cycle_members.join(", "))]
    DependencyCycle { cycle_members: Vec<String> },

    /// The dependency chain exceeds the maximum allowed depth.
    #[error("Dependency chain too deep: {depth} exceeds maximum of {max_depth}")]
    DependencyTooDeep { depth: usize, max_depth: usize },

    /// The task execution timed out.
    #[error("Task execution timed out after {timeout_seconds}s")]
    ExecutionTimeout { timeout_seconds: u32 },

    /// The task failed after exhausting all retry attempts.
    #[error("Task failed after {attempts} attempts: {last_error}")]
    RetriesExhausted { attempts: u8, last_error: String },

    /// An invalid cron expression was provided.
    #[error("Invalid cron expression: {expression}")]
    InvalidCronExpression { expression: String },

    /// A task dependency has not been satisfied.
    #[error("Dependency not satisfied: task {dependency_id} has not completed")]
    DependencyNotSatisfied { dependency_id: String },
}

// =============================================================================
// Plugin Errors
// =============================================================================

/// Errors related to the plugin system.
#[derive(Debug, Error)]
pub enum PluginError {
    /// The plugin manifest is invalid or missing required fields.
    #[error("Invalid manifest for plugin '{name}': {reason}")]
    InvalidManifest { name: String, reason: String },

    /// The plugin failed to load or initialize.
    #[error("Plugin '{name}' failed to load: {reason}")]
    LoadFailed { name: String, reason: String },

    /// A plugin attempted to register a name that is already taken.
    #[error("Name conflict: '{name}' already registered by plugin '{existing_plugin}'")]
    NameConflict {
        name: String,
        existing_plugin: String,
    },

    /// The plugin crashed or panicked during execution.
    #[error("Plugin '{name}' crashed: {reason}")]
    ExecutionFailed { name: String, reason: String },

    /// The plugin exceeded its resource limits.
    #[error("Plugin '{name}' exceeded resource limits: {resource}")]
    ResourceLimitExceeded { name: String, resource: String },
}

// =============================================================================
// Config Errors
// =============================================================================

/// Errors related to configuration loading and validation.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[error("Failed to read config file '{path}': {reason}")]
    FileReadError { path: String, reason: String },

    /// The configuration file contains invalid TOML syntax.
    #[error("Invalid TOML in '{path}': {reason}")]
    ParseError { path: String, reason: String },

    /// One or more configuration settings are invalid.
    #[error("Configuration validation failed: {}", errors.join("; "))]
    ValidationError { errors: Vec<String> },

    /// A required configuration value is missing.
    #[error("Missing required config value: {field}")]
    MissingField { field: String },

    /// An environment variable override has an invalid value.
    #[error("Invalid environment variable '{var}': {reason}")]
    InvalidEnvVar { var: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_error_from_llm_error() {
        let llm_err = LlmError::Timeout {
            provider: "openai".to_string(),
            elapsed_ms: 30000,
        };
        let platform_err: PlatformError = llm_err.into();
        assert!(matches!(platform_err, PlatformError::Llm(_)));
        assert!(platform_err.to_string().contains("openai"));
    }

    #[test]
    fn test_platform_error_from_security_error() {
        let sec_err = SecurityError::AuthenticationRequired;
        let platform_err: PlatformError = sec_err.into();
        assert!(matches!(platform_err, PlatformError::Security(_)));
        assert!(platform_err.to_string().contains("Authentication required"));
    }

    #[test]
    fn test_platform_error_from_store_error() {
        let store_err = StoreError::CapacityFull;
        let platform_err: PlatformError = store_err.into();
        assert!(matches!(platform_err, PlatformError::Storage(_)));
    }

    #[test]
    fn test_platform_error_from_task_error() {
        let task_err = TaskError::DependencyCycle {
            cycle_members: vec!["task_a".to_string(), "task_b".to_string()],
        };
        let platform_err: PlatformError = task_err.into();
        assert!(matches!(platform_err, PlatformError::Task(_)));
        assert!(platform_err.to_string().contains("task_a"));
    }

    #[test]
    fn test_platform_error_from_plugin_error() {
        let plugin_err = PluginError::NameConflict {
            name: "search".to_string(),
            existing_plugin: "core-tools".to_string(),
        };
        let platform_err: PlatformError = plugin_err.into();
        assert!(matches!(platform_err, PlatformError::Plugin(_)));
    }

    #[test]
    fn test_platform_error_from_config_error() {
        let config_err = ConfigError::ValidationError {
            errors: vec![
                "llm.providers: must have 1-10 providers".to_string(),
                "security.session_timeout: must be positive".to_string(),
            ],
        };
        let platform_err: PlatformError = config_err.into();
        assert!(matches!(platform_err, PlatformError::Config(_)));
        assert!(platform_err.to_string().contains("1-10 providers"));
    }

    #[test]
    fn test_llm_error_all_providers_failed() {
        let err = LlmError::AllProvidersFailed {
            attempts: vec![
                ProviderAttempt {
                    provider: "openai".to_string(),
                    error: "timeout".to_string(),
                    elapsed_ms: 30000,
                },
                ProviderAttempt {
                    provider: "anthropic".to_string(),
                    error: "rate limited".to_string(),
                    elapsed_ms: 100,
                },
            ],
        };
        assert!(err.to_string().contains("2 attempts"));
    }

    #[test]
    fn test_security_error_sandbox_violation() {
        let err = SecurityError::SandboxViolation {
            path: PathBuf::from("/etc/passwd"),
            access: AccessType::Read,
        };
        assert!(err.to_string().contains("/etc/passwd"));
        assert!(err.to_string().contains("Read"));
    }

    #[test]
    fn test_store_error_content_too_large() {
        let err = StoreError::ContentTooLarge {
            max: 10000,
            actual: 15000,
        };
        assert!(err.to_string().contains("15000"));
        assert!(err.to_string().contains("10000"));
    }

    #[test]
    fn test_task_error_dependency_cycle() {
        let err = TaskError::DependencyCycle {
            cycle_members: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        };
        assert!(err.to_string().contains("a, b, c"));
    }

    #[test]
    fn test_config_error_validation() {
        let err = ConfigError::ValidationError {
            errors: vec!["field1: invalid".to_string(), "field2: missing".to_string()],
        };
        assert!(err.to_string().contains("field1: invalid"));
        assert!(err.to_string().contains("field2: missing"));
    }
}
