//! Audit logging for the VPS AI Agent Platform.
//!
//! Records all security-relevant events including authentication attempts,
//! security violations (sandbox breaches, blocked IPs, denied commands),
//! and authorization decisions.
//!
//! Events are stored in an in-memory buffer with a bounded capacity.
//! When the memory-store is implemented, events will be persisted to the
//! `audit_log` SQLite table.
//!
//! The logger is async-friendly and thread-safe, suitable for concurrent
//! use from multiple request handlers.

use std::net::IpAddr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

/// The outcome of an audited action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    /// The action succeeded.
    Success,
    /// The action was denied or failed.
    Failure,
}

impl std::fmt::Display for AuditOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditOutcome::Success => write!(f, "success"),
            AuditOutcome::Failure => write!(f, "failure"),
        }
    }
}

/// A single audit event recording a security-relevant action.
///
/// Corresponds to a row in the `audit_log` SQLite table:
/// - `id`: unique event identifier
/// - `timestamp`: ISO 8601 timestamp
/// - `source_ip`: originating IP address (if available)
/// - `user_id`: authenticated user (if known)
/// - `action`: description of what happened
/// - `outcome`: success or failure
/// - `details`: optional JSON with additional context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique identifier for this audit event.
    pub id: String,
    /// ISO 8601 timestamp of when the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Source IP address of the request, if available.
    pub source_ip: Option<IpAddr>,
    /// User ID of the authenticated user, if known.
    pub user_id: Option<String>,
    /// Human-readable description of the action.
    pub action: String,
    /// Whether the action succeeded or failed.
    pub outcome: AuditOutcome,
    /// Optional JSON details providing additional context.
    pub details: Option<serde_json::Value>,
}

impl AuditEvent {
    /// Create a new audit event with the current timestamp.
    pub fn new(
        source_ip: Option<IpAddr>,
        user_id: Option<String>,
        action: impl Into<String>,
        outcome: AuditOutcome,
        details: Option<serde_json::Value>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            source_ip,
            user_id,
            action: action.into(),
            outcome,
            details,
        }
    }
}

/// Configuration for the audit logger.
#[derive(Debug, Clone)]
pub struct AuditLoggerConfig {
    /// Maximum number of events to retain in the in-memory buffer.
    /// When exceeded, the oldest events are discarded.
    pub max_buffer_size: usize,
}

impl Default for AuditLoggerConfig {
    fn default() -> Self {
        Self {
            max_buffer_size: 10_000,
        }
    }
}

/// Thread-safe audit logger that records security events.
///
/// Events are stored in a bounded in-memory ring buffer. When the
/// memory-store crate is ready, this will be extended to persist
/// events to the `audit_log` SQLite table.
#[derive(Debug, Clone)]
pub struct AuditLogger {
    config: AuditLoggerConfig,
    events: Arc<RwLock<Vec<AuditEvent>>>,
}

impl AuditLogger {
    /// Create a new audit logger with the given configuration.
    pub fn new(config: AuditLoggerConfig) -> Self {
        Self {
            config,
            events: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Record a generic audit event.
    pub async fn log_event(&self, event: AuditEvent) {
        info!(
            action = %event.action,
            outcome = %event.outcome,
            source_ip = ?event.source_ip,
            user_id = ?event.user_id,
            "Audit event recorded"
        );

        let mut events = self.events.write().await;
        events.push(event);

        // Evict oldest events if buffer is full
        if events.len() > self.config.max_buffer_size {
            let overflow = events.len() - self.config.max_buffer_size;
            events.drain(0..overflow);
        }
    }

    /// Log an authentication attempt.
    ///
    /// Records the timestamp, source IP, username or API key identifier,
    /// and whether the attempt succeeded or failed.
    pub async fn log_auth_attempt(
        &self,
        source_ip: IpAddr,
        identifier: &str,
        outcome: AuditOutcome,
    ) {
        let action = match outcome {
            AuditOutcome::Success => format!("auth.login_success: {}", identifier),
            AuditOutcome::Failure => format!("auth.login_failure: {}", identifier),
        };

        let details = serde_json::json!({
            "identifier": identifier,
            "auth_type": if identifier.starts_with("key_") { "api_key" } else { "password" },
        });

        let event = AuditEvent::new(
            Some(source_ip),
            None, // user_id not yet known at auth time
            action,
            outcome,
            Some(details),
        );

        if outcome == AuditOutcome::Failure {
            warn!(
                source_ip = %source_ip,
                identifier = %identifier,
                "Authentication attempt failed"
            );
        }

        self.log_event(event).await;
    }

    /// Log an IP being blocked due to brute-force protection.
    pub async fn log_ip_blocked(&self, source_ip: IpAddr, reason: &str) {
        let event = AuditEvent::new(
            Some(source_ip),
            None,
            "security.ip_blocked",
            AuditOutcome::Failure,
            Some(serde_json::json!({
                "reason": reason,
                "violation_type": "brute_force",
            })),
        );

        warn!(
            source_ip = %source_ip,
            reason = %reason,
            "IP blocked — security violation"
        );

        self.log_event(event).await;
    }

    /// Log a sandbox breach attempt (filesystem access outside boundaries).
    pub async fn log_sandbox_breach(
        &self,
        source_ip: Option<IpAddr>,
        user_id: Option<&str>,
        path: &str,
        access_type: &str,
    ) {
        let event = AuditEvent::new(
            source_ip,
            user_id.map(|s| s.to_string()),
            "security.sandbox_breach",
            AuditOutcome::Failure,
            Some(serde_json::json!({
                "path": path,
                "access_type": access_type,
                "violation_type": "sandbox",
            })),
        );

        warn!(
            path = %path,
            access_type = %access_type,
            user_id = ?user_id,
            "Sandbox breach attempt"
        );

        self.log_event(event).await;
    }

    /// Log a denied command execution attempt.
    pub async fn log_command_denied(
        &self,
        source_ip: Option<IpAddr>,
        user_id: Option<&str>,
        command: &str,
        reason: &str,
    ) {
        let event = AuditEvent::new(
            source_ip,
            user_id.map(|s| s.to_string()),
            "security.command_denied",
            AuditOutcome::Failure,
            Some(serde_json::json!({
                "command": command,
                "reason": reason,
                "violation_type": "command_policy",
            })),
        );

        warn!(
            command = %command,
            reason = %reason,
            user_id = ?user_id,
            "Command execution denied"
        );

        self.log_event(event).await;
    }

    /// Log a network access violation (connection to disallowed host/port).
    pub async fn log_network_violation(
        &self,
        source_ip: Option<IpAddr>,
        user_id: Option<&str>,
        target_host: &str,
        target_port: u16,
    ) {
        let event = AuditEvent::new(
            source_ip,
            user_id.map(|s| s.to_string()),
            "security.network_blocked",
            AuditOutcome::Failure,
            Some(serde_json::json!({
                "target_host": target_host,
                "target_port": target_port,
                "violation_type": "network_policy",
            })),
        );

        warn!(
            target_host = %target_host,
            target_port = %target_port,
            user_id = ?user_id,
            "Network access blocked"
        );

        self.log_event(event).await;
    }

    /// Get all recorded events (for testing and querying).
    pub async fn get_events(&self) -> Vec<AuditEvent> {
        self.events.read().await.clone()
    }

    /// Get the number of recorded events.
    pub async fn event_count(&self) -> usize {
        self.events.read().await.len()
    }

    /// Clear all recorded events (primarily for testing).
    pub async fn clear(&self) {
        self.events.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100))
    }

    #[tokio::test]
    async fn test_log_auth_success() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_auth_attempt(test_ip(), "admin", AuditOutcome::Success)
            .await;

        let events = logger.get_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome, AuditOutcome::Success);
        assert!(events[0].action.contains("auth.login_success"));
        assert!(events[0].action.contains("admin"));
        assert_eq!(events[0].source_ip, Some(test_ip()));
    }

    #[tokio::test]
    async fn test_log_auth_failure() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_auth_attempt(test_ip(), "unknown_user", AuditOutcome::Failure)
            .await;

        let events = logger.get_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome, AuditOutcome::Failure);
        assert!(events[0].action.contains("auth.login_failure"));
        assert!(events[0].action.contains("unknown_user"));
    }

    #[tokio::test]
    async fn test_log_ip_blocked() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_ip_blocked(test_ip(), "5 consecutive failures")
            .await;

        let events = logger.get_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "security.ip_blocked");
        assert_eq!(events[0].outcome, AuditOutcome::Failure);

        let details = events[0].details.as_ref().unwrap();
        assert_eq!(details["reason"], "5 consecutive failures");
        assert_eq!(details["violation_type"], "brute_force");
    }

    #[tokio::test]
    async fn test_log_sandbox_breach() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_sandbox_breach(Some(test_ip()), Some("user-123"), "/etc/passwd", "read")
            .await;

        let events = logger.get_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "security.sandbox_breach");
        assert_eq!(events[0].user_id, Some("user-123".to_string()));

        let details = events[0].details.as_ref().unwrap();
        assert_eq!(details["path"], "/etc/passwd");
        assert_eq!(details["access_type"], "read");
        assert_eq!(details["violation_type"], "sandbox");
    }

    #[tokio::test]
    async fn test_log_command_denied() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_command_denied(
                Some(test_ip()),
                Some("user-456"),
                "rm -rf /",
                "command on blocklist",
            )
            .await;

        let events = logger.get_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "security.command_denied");

        let details = events[0].details.as_ref().unwrap();
        assert_eq!(details["command"], "rm -rf /");
        assert_eq!(details["reason"], "command on blocklist");
        assert_eq!(details["violation_type"], "command_policy");
    }

    #[tokio::test]
    async fn test_log_network_violation() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_network_violation(None, Some("user-789"), "evil.example.com", 443)
            .await;

        let events = logger.get_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "security.network_blocked");
        assert_eq!(events[0].source_ip, None);

        let details = events[0].details.as_ref().unwrap();
        assert_eq!(details["target_host"], "evil.example.com");
        assert_eq!(details["target_port"], 443);
        assert_eq!(details["violation_type"], "network_policy");
    }

    #[tokio::test]
    async fn test_buffer_eviction() {
        let config = AuditLoggerConfig { max_buffer_size: 3 };
        let logger = AuditLogger::new(config);

        // Log 5 events — only the last 3 should remain
        for i in 0..5 {
            let event = AuditEvent::new(
                Some(test_ip()),
                None,
                format!("action_{}", i),
                AuditOutcome::Success,
                None,
            );
            logger.log_event(event).await;
        }

        let events = logger.get_events().await;
        assert_eq!(events.len(), 3);
        // Oldest events (0, 1) should have been evicted
        assert!(events[0].action.contains("action_2"));
        assert!(events[1].action.contains("action_3"));
        assert!(events[2].action.contains("action_4"));
    }

    #[tokio::test]
    async fn test_event_count() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        assert_eq!(logger.event_count().await, 0);

        logger
            .log_auth_attempt(test_ip(), "user", AuditOutcome::Success)
            .await;
        assert_eq!(logger.event_count().await, 1);

        logger
            .log_auth_attempt(test_ip(), "user", AuditOutcome::Failure)
            .await;
        assert_eq!(logger.event_count().await, 2);
    }

    #[tokio::test]
    async fn test_clear_events() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_auth_attempt(test_ip(), "user", AuditOutcome::Success)
            .await;
        assert_eq!(logger.event_count().await, 1);

        logger.clear().await;
        assert_eq!(logger.event_count().await, 0);
    }

    #[tokio::test]
    async fn test_event_has_valid_id_and_timestamp() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_auth_attempt(test_ip(), "user", AuditOutcome::Success)
            .await;

        let events = logger.get_events().await;
        let event = &events[0];

        // ID should be a valid UUID
        assert!(Uuid::parse_str(&event.id).is_ok());
        // Timestamp should be recent (within last second)
        let now = Utc::now();
        let diff = now - event.timestamp;
        assert!(diff.num_seconds() < 2);
    }

    #[tokio::test]
    async fn test_api_key_auth_type_detection() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_auth_attempt(test_ip(), "key_abc123", AuditOutcome::Success)
            .await;

        let events = logger.get_events().await;
        let details = events[0].details.as_ref().unwrap();
        assert_eq!(details["auth_type"], "api_key");
    }

    #[tokio::test]
    async fn test_password_auth_type_detection() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());

        logger
            .log_auth_attempt(test_ip(), "admin", AuditOutcome::Success)
            .await;

        let events = logger.get_events().await;
        let details = events[0].details.as_ref().unwrap();
        assert_eq!(details["auth_type"], "password");
    }

    #[tokio::test]
    async fn test_concurrent_logging() {
        let logger = AuditLogger::new(AuditLoggerConfig::default());
        let mut handles = Vec::new();

        for i in 0..10 {
            let logger_clone = logger.clone();
            let handle = tokio::spawn(async move {
                logger_clone
                    .log_auth_attempt(test_ip(), &format!("user_{}", i), AuditOutcome::Success)
                    .await;
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(logger.event_count().await, 10);
    }

    #[tokio::test]
    async fn test_audit_event_serialization() {
        let event = AuditEvent::new(
            Some(test_ip()),
            Some("user-123".to_string()),
            "auth.login_success",
            AuditOutcome::Success,
            Some(serde_json::json!({"key": "value"})),
        );

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: AuditEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.action, "auth.login_success");
        assert_eq!(deserialized.outcome, AuditOutcome::Success);
        assert_eq!(deserialized.user_id, Some("user-123".to_string()));
    }
}
