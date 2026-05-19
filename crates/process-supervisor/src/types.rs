//! Types for the process supervisor.
//!
//! Defines configuration, error types, health status, and statistics
//! structures used throughout the supervisor crate.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Configuration for the process supervisor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorConfig {
    /// Interval between health check pings (default: 15 seconds).
    pub health_check_interval: Duration,

    /// Maximum time without a health response before triggering restart (default: 60 seconds).
    pub unresponsive_timeout: Duration,

    /// Maximum time to wait before restarting after unexpected termination (default: 10 seconds).
    pub restart_delay: Duration,

    /// Maximum time to auto-start after OS boot (default: 30 seconds).
    pub boot_start_timeout: Duration,

    /// Maximum number of restart events to retain in history.
    pub max_restart_history: usize,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            health_check_interval: Duration::from_secs(15),
            unresponsive_timeout: Duration::from_secs(60),
            restart_delay: Duration::from_secs(5),
            boot_start_timeout: Duration::from_secs(30),
            max_restart_history: 10,
        }
    }
}

/// Errors that can occur during supervisor operations.
#[derive(Debug, Error)]
pub enum SupervisorError {
    /// The supervisor is already running.
    #[error("Supervisor is already running")]
    AlreadyRunning,

    /// The supervisor is not running.
    #[error("Supervisor is not running")]
    NotRunning,

    /// Failed to start the agent.
    #[error("Failed to start agent: {reason}")]
    AgentStartFailed { reason: String },

    /// Failed to restart the agent.
    #[error("Failed to restart agent: {reason}")]
    RestartFailed { reason: String },

    /// Failed to restore session state.
    #[error("Failed to restore session state: {reason}")]
    StateRestoreFailed { reason: String },

    /// Health check failed.
    #[error("Health check failed: {reason}")]
    HealthCheckFailed { reason: String },
}

/// The health status of the supervised agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Agent is healthy and responsive.
    Healthy,
    /// Agent is degraded (responding slowly or partially).
    Degraded { reason: String },
    /// Agent is unresponsive (no response within timeout).
    Unresponsive { since: DateTime<Utc> },
    /// Agent has terminated unexpectedly.
    Terminated,
    /// Agent is starting up.
    Starting,
    /// Supervisor is not monitoring (stopped).
    Unknown,
}

/// Reason for an agent restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestartReason {
    /// Agent was unresponsive for longer than the configured timeout.
    Unresponsive { duration_seconds: u64 },
    /// Agent process terminated unexpectedly.
    UnexpectedTermination,
    /// Manual restart requested by operator.
    Manual { reason: String },
    /// Initial startup (first start or after OS boot).
    InitialStart,
    /// Agent reported an error state.
    ErrorState { message: String },
}

impl std::fmt::Display for RestartReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestartReason::Unresponsive { duration_seconds } => {
                write!(f, "unresponsive for {}s", duration_seconds)
            }
            RestartReason::UnexpectedTermination => write!(f, "unexpected termination"),
            RestartReason::Manual { reason } => write!(f, "manual: {}", reason),
            RestartReason::InitialStart => write!(f, "initial start"),
            RestartReason::ErrorState { message } => write!(f, "error state: {}", message),
        }
    }
}

/// A recorded restart event with timestamp and reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartEvent {
    /// When the restart occurred.
    pub timestamp: DateTime<Utc>,
    /// Why the restart was triggered.
    pub reason: RestartReason,
    /// Whether the restart was successful.
    pub success: bool,
    /// How long the restart took.
    pub duration_ms: u64,
}

/// Statistics about the supervisor's operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorStats {
    /// Total uptime since the supervisor started.
    pub uptime: Duration,
    /// Total number of restarts performed.
    pub restart_count: u32,
    /// Timestamp of the last restart (if any).
    pub last_restart: Option<DateTime<Utc>>,
    /// Reason for the last restart (if any).
    pub last_restart_reason: Option<String>,
    /// History of the last N restart events.
    pub restart_history: Vec<RestartEvent>,
    /// Current health status of the agent.
    pub current_health: HealthStatus,
}
