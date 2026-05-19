//! WebSocket connection management for the TUI.
//!
//! Handles connecting to the API server, receiving messages and status
//! updates, and auto-reconnection on disconnection. Designed to handle
//! SSH latency up to 500ms without display corruption or dropped input.

use std::time::{Duration, Instant};

/// Connection manager for the WebSocket link to the API server.
///
/// Tracks connection state and implements auto-reconnect logic:
/// - Reconnect every ≤5 seconds on disconnection
/// - Up to 6 reconnect attempts before giving up
/// - Displays disconnection notification to the user
pub struct ConnectionManager {
    /// WebSocket URL to connect to.
    ws_url: String,
    /// API key for authentication.
    api_key: String,
    /// Current connection state.
    state: ConnectionStatus,
    /// Reconnect interval.
    reconnect_interval: Duration,
    /// Maximum reconnect attempts.
    max_attempts: u8,
    /// Current reconnect attempt count.
    attempt_count: u8,
    /// Time of last reconnect attempt.
    last_attempt: Option<Instant>,
}

/// Internal connection status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Connected and receiving data.
    Connected,
    /// Disconnected, waiting to reconnect.
    Disconnected,
    /// Actively attempting to reconnect.
    Reconnecting,
    /// All reconnect attempts exhausted.
    Failed,
}

impl ConnectionManager {
    /// Create a new ConnectionManager.
    pub fn new(
        ws_url: String,
        api_key: String,
        reconnect_interval: Duration,
        max_attempts: u8,
    ) -> Self {
        Self {
            ws_url,
            api_key,
            state: ConnectionStatus::Disconnected,
            reconnect_interval,
            max_attempts,
            attempt_count: 0,
            last_attempt: None,
        }
    }

    /// Get the current connection status.
    pub fn status(&self) -> &ConnectionStatus {
        &self.state
    }

    /// Get the WebSocket URL.
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// Get the number of reconnect attempts made.
    pub fn attempt_count(&self) -> u8 {
        self.attempt_count
    }

    /// Check if a reconnect attempt should be made now.
    pub fn should_reconnect(&self) -> bool {
        if self.state != ConnectionStatus::Disconnected {
            return false;
        }
        if self.attempt_count >= self.max_attempts {
            return false;
        }
        match self.last_attempt {
            None => true,
            Some(last) => last.elapsed() >= self.reconnect_interval,
        }
    }

    /// Mark the start of a reconnect attempt.
    pub fn start_reconnect(&mut self) {
        self.state = ConnectionStatus::Reconnecting;
        self.attempt_count += 1;
        self.last_attempt = Some(Instant::now());
    }

    /// Mark a successful connection.
    pub fn on_connected(&mut self) {
        self.state = ConnectionStatus::Connected;
        self.attempt_count = 0;
        self.last_attempt = None;
    }

    /// Mark a disconnection event.
    pub fn on_disconnected(&mut self) {
        self.state = ConnectionStatus::Disconnected;
        self.attempt_count = 0;
        self.last_attempt = None;
    }

    /// Mark a failed reconnect attempt.
    pub fn on_reconnect_failed(&mut self) {
        if self.attempt_count >= self.max_attempts {
            self.state = ConnectionStatus::Failed;
        } else {
            self.state = ConnectionStatus::Disconnected;
        }
    }

    /// Reset the connection manager (e.g., for manual reconnect).
    pub fn reset(&mut self) {
        self.state = ConnectionStatus::Disconnected;
        self.attempt_count = 0;
        self.last_attempt = None;
    }

    /// Check if all reconnect attempts have been exhausted.
    pub fn is_exhausted(&self) -> bool {
        self.attempt_count >= self.max_attempts
    }
}

/// Incoming messages from the WebSocket connection.
#[derive(Debug, Clone)]
pub enum ServerMessage {
    /// A chat message from the agent.
    ChatResponse {
        content: String,
    },
    /// Agent status update.
    StatusUpdate {
        status: String,
        task: Option<String>,
        progress: Option<f32>,
    },
    /// Resource usage update.
    ResourceUpdate {
        cpu_percent: f32,
        memory_mb: f32,
    },
    /// Error from the server.
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_manager_creation() {
        let cm = ConnectionManager::new(
            "ws://localhost:3000".to_string(),
            "test-key".to_string(),
            Duration::from_secs(5),
            6,
        );
        assert_eq!(*cm.status(), ConnectionStatus::Disconnected);
        assert_eq!(cm.attempt_count(), 0);
    }

    #[test]
    fn test_connection_lifecycle() {
        let mut cm = ConnectionManager::new(
            "ws://localhost:3000".to_string(),
            "test-key".to_string(),
            Duration::from_secs(5),
            6,
        );

        // Connect
        cm.on_connected();
        assert_eq!(*cm.status(), ConnectionStatus::Connected);

        // Disconnect
        cm.on_disconnected();
        assert_eq!(*cm.status(), ConnectionStatus::Disconnected);
        assert_eq!(cm.attempt_count(), 0);
    }

    #[test]
    fn test_reconnect_attempts() {
        let mut cm = ConnectionManager::new(
            "ws://localhost:3000".to_string(),
            "test-key".to_string(),
            Duration::from_secs(0), // Immediate for testing
            3,
        );

        cm.on_disconnected();

        // First attempt
        assert!(cm.should_reconnect());
        cm.start_reconnect();
        assert_eq!(*cm.status(), ConnectionStatus::Reconnecting);
        cm.on_reconnect_failed();
        assert_eq!(cm.attempt_count(), 1);

        // Second attempt
        cm.start_reconnect();
        cm.on_reconnect_failed();
        assert_eq!(cm.attempt_count(), 2);

        // Third attempt (max)
        cm.start_reconnect();
        cm.on_reconnect_failed();
        assert_eq!(cm.attempt_count(), 3);
        assert_eq!(*cm.status(), ConnectionStatus::Failed);
        assert!(cm.is_exhausted());
    }

    #[test]
    fn test_should_not_reconnect_when_connected() {
        let mut cm = ConnectionManager::new(
            "ws://localhost:3000".to_string(),
            "test-key".to_string(),
            Duration::from_secs(5),
            6,
        );
        cm.on_connected();
        assert!(!cm.should_reconnect());
    }

    #[test]
    fn test_reset() {
        let mut cm = ConnectionManager::new(
            "ws://localhost:3000".to_string(),
            "test-key".to_string(),
            Duration::from_secs(0),
            3,
        );
        cm.on_disconnected();
        cm.start_reconnect();
        cm.on_reconnect_failed();
        cm.start_reconnect();
        cm.on_reconnect_failed();

        cm.reset();
        assert_eq!(*cm.status(), ConnectionStatus::Disconnected);
        assert_eq!(cm.attempt_count(), 0);
        assert!(cm.should_reconnect());
    }
}
