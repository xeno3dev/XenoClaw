//! SIGHUP signal handler for zero-downtime configuration reload.
//!
//! This module provides a tokio-based signal handler that listens for SIGHUP
//! and triggers a configuration reload using the [`reload`](super::reload) module.
//!
//! # Usage
//!
//! ```rust,no_run
//! use std::path::PathBuf;
//! use std::sync::Arc;
//! use tokio::sync::RwLock;
//! use common::config::{PlatformConfig, signal::spawn_reload_handler};
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = Arc::new(RwLock::new(PlatformConfig::default()));
//!     let config_path = PathBuf::from("/etc/xenoclaw/config.toml");
//!     spawn_reload_handler(config, config_path);
//! }
//! ```
//!
//! # Reloadable vs Non-Reloadable Settings
//!
//! ## Hot-reloadable (applied on SIGHUP):
//! - `api.rate_limit_per_minute` — API rate limiting
//! - `monitoring.log_level` — Log verbosity
//! - `monitoring.log_retention_days` — Log retention period
//! - `monitoring.max_log_file_size_mb` — Log rotation threshold
//! - `monitoring.metrics_enabled` — Metrics collection toggle
//! - `monitoring.alert_rules` — Alert rule definitions
//! - `plugins.enabled` — Plugin system toggle
//! - `mcp.server_enabled` — MCP server toggle (enables/disables the server)
//! - `mcp.servers` — External MCP server list (triggers reconnection)
//!
//! ## Require full restart:
//! - `api.host`, `api.port` — API server bind address
//! - `web.host` — Web server host (dashboard now served on the API port)
//! - `monitoring.metrics_port` — Metrics endpoint port
//! - `general.data_dir`, `general.log_dir` — Data directories
//! - `llm.providers` — LLM provider configurations
//! - `security.*` — All security settings (filesystem rules, network allowlist, resource limits)
//! - `security.admin_username`, `security.admin_password_hash` — Admin credentials (security — require restart)
//! - `mcp.server_transport` — MCP server transport type (requires restart)
//! - `mcp.server_port` — MCP server port (requires restart)
//! - TLS certificates and authentication keys
//! - Database path and connection settings
//!
//! Requirements: 12.6

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info};

use super::models::PlatformConfig;
use super::reload::{apply_reload, reload_config};

/// Spawn a background task that listens for SIGHUP and reloads configuration.
///
/// This function registers a Unix signal handler for SIGHUP. When the signal
/// is received, it:
/// 1. Reads the configuration file from `config_path`
/// 2. Extracts only the hot-reloadable settings
/// 3. Validates them
/// 4. Applies the changes to the shared `config`
///
/// If any step fails, the error is logged and the current configuration
/// remains unchanged (safe reload semantics).
///
/// # Platform Support
///
/// This function is only available on Unix platforms. On non-Unix platforms,
/// it logs a warning and returns without spawning a handler.
///
/// # Panics
///
/// Does not panic. All errors are logged and the handler continues listening
/// for subsequent signals.
#[cfg(unix)]
pub fn spawn_reload_handler(config: Arc<RwLock<PlatformConfig>>, config_path: PathBuf) {
    tokio::spawn(async move {
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .expect("failed to register SIGHUP handler");

        info!(
            config_path = %config_path.display(),
            "SIGHUP reload handler registered"
        );

        loop {
            signal.recv().await;
            info!("Received SIGHUP, reloading configuration...");

            match reload_config(&config_path) {
                Ok(reloadable) => {
                    let mut cfg = config.write().await;
                    apply_reload(&mut cfg, reloadable);
                    info!("Configuration reload completed successfully");
                }
                Err(e) => {
                    error!(
                        error = %e,
                        "Configuration reload failed, keeping current settings"
                    );
                }
            }
        }
    });
}

/// No-op on non-Unix platforms. Logs a warning that SIGHUP reload is unavailable.
#[cfg(not(unix))]
pub fn spawn_reload_handler(_config: Arc<RwLock<PlatformConfig>>, _config_path: PathBuf) {
    tracing::warn!("SIGHUP reload handler is not available on this platform");
}

/// Trigger a manual configuration reload (useful for API-driven reloads).
///
/// This performs the same operation as a SIGHUP signal but can be called
/// programmatically (e.g., from a `PUT /api/v1/config/reload` endpoint).
///
/// Returns `Ok(())` if the reload succeeded, or an error description if it failed.
pub async fn trigger_reload(
    config: &Arc<RwLock<PlatformConfig>>,
    config_path: &PathBuf,
) -> Result<(), String> {
    info!("Manual configuration reload triggered");

    match reload_config(config_path) {
        Ok(reloadable) => {
            let mut cfg = config.write().await;
            apply_reload(&mut cfg, reloadable);
            info!("Manual configuration reload completed successfully");
            Ok(())
        }
        Err(e) => {
            let msg = format!("Configuration reload failed: {e}");
            error!("{}", msg);
            Err(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_trigger_reload_valid_config() {
        let toml_content = r#"
[general]
agent_name = "test-agent"

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
rate_limit_per_minute = 500

[monitoring]
log_level = "debug"
log_retention_days = 14
max_log_file_size_mb = 100
metrics_enabled = true
metrics_port = 9100

[plugins]
enabled = false
"#;

        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(toml_content.as_bytes()).unwrap();
        let path = tmp.path().to_path_buf();

        // Create a default config to reload into
        let config = Arc::new(RwLock::new(PlatformConfig::default()));

        let result = trigger_reload(&config, &path).await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result.err());

        let cfg = config.read().await;
        assert_eq!(cfg.api.rate_limit_per_minute, 500);
        assert_eq!(cfg.monitoring.log_retention_days, 14);
    }

    #[tokio::test]
    async fn test_trigger_reload_invalid_config() {
        let toml_content = r#"
[general]
agent_name = "test-agent"

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

        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(toml_content.as_bytes()).unwrap();
        let path = tmp.path().to_path_buf();

        let config = Arc::new(RwLock::new(PlatformConfig::default()));
        let original_rate = config.read().await.api.rate_limit_per_minute;

        let result = trigger_reload(&config, &path).await;
        assert!(result.is_err());

        // Config should remain unchanged
        let cfg = config.read().await;
        assert_eq!(cfg.api.rate_limit_per_minute, original_rate);
    }

    #[tokio::test]
    async fn test_trigger_reload_missing_file() {
        let path = PathBuf::from("/nonexistent/config.toml");
        let config = Arc::new(RwLock::new(PlatformConfig::default()));

        let result = trigger_reload(&config, &path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("reload failed"));
    }
}
