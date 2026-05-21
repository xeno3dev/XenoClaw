//! Health monitoring for the supervised agent.
//!
//! Provides the health check mechanism that determines whether the agent
//! is responsive. The agent is considered healthy if it responds to a
//! status query within a reasonable time frame.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::RwLock;
use tokio::time;
use tracing::{debug, warn};

use crate::types::HealthStatus;

/// Tracks the health state of the supervised agent.
///
/// The health monitor records when the last successful health check occurred
/// and determines the current health status based on elapsed time since
/// the last response.
pub struct HealthMonitor {
    /// Timestamp of the last successful health check response.
    last_healthy_response: Arc<RwLock<Option<Instant>>>,

    /// Maximum duration without a response before declaring unresponsive.
    unresponsive_timeout: Duration,

    /// Whether the agent is currently considered alive (not terminated).
    agent_alive: Arc<RwLock<bool>>,
}

impl HealthMonitor {
    /// Create a new health monitor with the given unresponsive timeout.
    pub fn new(unresponsive_timeout: Duration) -> Self {
        Self {
            last_healthy_response: Arc::new(RwLock::new(None)),
            unresponsive_timeout,
            agent_alive: Arc::new(RwLock::new(false)),
        }
    }

    /// Record a successful health check response.
    ///
    /// This should be called whenever the agent responds to a health ping.
    pub async fn record_healthy_response(&self) {
        let mut last = self.last_healthy_response.write().await;
        *last = Some(Instant::now());
    }

    /// Mark the agent as alive (started and running).
    pub async fn mark_alive(&self) {
        *self.agent_alive.write().await = true;
        self.record_healthy_response().await;
    }

    /// Mark the agent as terminated.
    pub async fn mark_terminated(&self) {
        *self.agent_alive.write().await = false;
    }

    /// Check if the agent is currently marked as alive.
    pub async fn is_alive(&self) -> bool {
        *self.agent_alive.read().await
    }

    /// Get the current health status based on the last response time.
    pub async fn current_status(&self) -> HealthStatus {
        let alive = *self.agent_alive.read().await;
        if !alive {
            return HealthStatus::Terminated;
        }

        let last_response = *self.last_healthy_response.read().await;
        match last_response {
            None => HealthStatus::Starting,
            Some(last) => {
                let elapsed = last.elapsed();
                if elapsed <= self.unresponsive_timeout {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unresponsive {
                        since: Utc::now() - chrono::Duration::from_std(elapsed).unwrap_or_default(),
                    }
                }
            }
        }
    }

    /// Check if the agent is unresponsive (no response within timeout).
    pub async fn is_unresponsive(&self) -> bool {
        matches!(
            self.current_status().await,
            HealthStatus::Unresponsive { .. }
        )
    }

    /// Get the duration since the last healthy response.
    pub async fn time_since_last_response(&self) -> Option<Duration> {
        let last = *self.last_healthy_response.read().await;
        last.map(|t| t.elapsed())
    }

    /// Perform a health check against the agent using the provided check function.
    ///
    /// The check function should return `true` if the agent is responsive.
    /// This method has a built-in timeout of 5 seconds for the check itself.
    pub async fn perform_check<F, Fut>(&self, check_fn: F) -> HealthStatus
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let alive = *self.agent_alive.read().await;
        if !alive {
            return HealthStatus::Terminated;
        }

        // Run the health check with a 5-second timeout
        let check_timeout = Duration::from_secs(5);
        let result = time::timeout(check_timeout, check_fn()).await;

        match result {
            Ok(true) => {
                self.record_healthy_response().await;
                debug!("Health check passed");
                HealthStatus::Healthy
            }
            Ok(false) => {
                warn!("Health check returned unhealthy");
                // Don't update last_healthy_response — let the timeout logic handle it
                self.current_status().await
            }
            Err(_) => {
                warn!("Health check timed out (5s)");
                self.current_status().await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_new_monitor_starts_unknown() {
        let monitor = HealthMonitor::new(Duration::from_secs(60));
        let status = monitor.current_status().await;
        // Not alive yet, so terminated
        assert_eq!(status, HealthStatus::Terminated);
    }

    #[tokio::test]
    async fn test_mark_alive_then_healthy() {
        let monitor = HealthMonitor::new(Duration::from_secs(60));
        monitor.mark_alive().await;
        let status = monitor.current_status().await;
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_mark_terminated() {
        let monitor = HealthMonitor::new(Duration::from_secs(60));
        monitor.mark_alive().await;
        monitor.mark_terminated().await;
        let status = monitor.current_status().await;
        assert_eq!(status, HealthStatus::Terminated);
    }

    #[tokio::test]
    async fn test_unresponsive_after_timeout() {
        // Use a very short timeout for testing
        let monitor = HealthMonitor::new(Duration::from_millis(50));
        monitor.mark_alive().await;

        // Wait for the timeout to elapse
        time::sleep(Duration::from_millis(100)).await;

        let status = monitor.current_status().await;
        assert!(matches!(status, HealthStatus::Unresponsive { .. }));
        assert!(monitor.is_unresponsive().await);
    }

    #[tokio::test]
    async fn test_healthy_after_response() {
        let monitor = HealthMonitor::new(Duration::from_millis(50));
        monitor.mark_alive().await;

        // Wait a bit but record a response before timeout
        time::sleep(Duration::from_millis(30)).await;
        monitor.record_healthy_response().await;

        let status = monitor.current_status().await;
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_perform_check_healthy() {
        let monitor = HealthMonitor::new(Duration::from_secs(60));
        monitor.mark_alive().await;

        let status = monitor.perform_check(|| async { true }).await;
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_perform_check_unhealthy() {
        let monitor = HealthMonitor::new(Duration::from_millis(50));
        monitor.mark_alive().await;

        // Let the timeout elapse
        time::sleep(Duration::from_millis(100)).await;

        let status = monitor.perform_check(|| async { false }).await;
        assert!(matches!(status, HealthStatus::Unresponsive { .. }));
    }

    #[tokio::test]
    async fn test_time_since_last_response() {
        let monitor = HealthMonitor::new(Duration::from_secs(60));

        // No response yet
        assert!(monitor.time_since_last_response().await.is_none());

        monitor.mark_alive().await;
        time::sleep(Duration::from_millis(10)).await;

        let elapsed = monitor.time_since_last_response().await.unwrap();
        assert!(elapsed >= Duration::from_millis(10));
    }
}
