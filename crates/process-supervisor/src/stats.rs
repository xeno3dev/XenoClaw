//! Statistics tracking for the process supervisor.
//!
//! Maintains uptime, restart count, last restart timestamp/reason,
//! and a bounded history of the last N restart events.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use crate::types::{HealthStatus, RestartEvent, RestartReason, SupervisorStats};

/// Tracks supervisor statistics including uptime and restart history.
///
/// Thread-safe via internal RwLock for concurrent access from the
/// monitoring loop and external stats queries.
pub struct StatsTracker {
    /// When the supervisor was started.
    started_at: Instant,

    /// Total number of restarts performed.
    restart_count: RwLock<u32>,

    /// Timestamp of the last restart.
    last_restart: RwLock<Option<DateTime<Utc>>>,

    /// Reason for the last restart.
    last_restart_reason: RwLock<Option<RestartReason>>,

    /// Bounded history of restart events.
    restart_history: RwLock<VecDeque<RestartEvent>>,

    /// Maximum number of events to retain.
    max_history: usize,
}

impl StatsTracker {
    /// Create a new stats tracker with the given history capacity.
    pub fn new(max_history: usize) -> Self {
        Self {
            started_at: Instant::now(),
            restart_count: RwLock::new(0),
            last_restart: RwLock::new(None),
            last_restart_reason: RwLock::new(None),
            restart_history: RwLock::new(VecDeque::with_capacity(max_history)),
            max_history,
        }
    }

    /// Record a restart event.
    ///
    /// Increments the restart count, updates the last restart info,
    /// and appends to the bounded history (evicting oldest if full).
    pub async fn record_restart(&self, reason: RestartReason, success: bool, duration_ms: u64) {
        let event = RestartEvent {
            timestamp: Utc::now(),
            reason: reason.clone(),
            success,
            duration_ms,
        };

        // Update restart count
        {
            let mut count = self.restart_count.write().await;
            *count += 1;
        }

        // Update last restart info
        {
            let mut last = self.last_restart.write().await;
            *last = Some(event.timestamp);
        }
        {
            let mut last_reason = self.last_restart_reason.write().await;
            *last_reason = Some(reason);
        }

        // Append to history (bounded)
        {
            let mut history = self.restart_history.write().await;
            if history.len() >= self.max_history {
                history.pop_front();
            }
            history.push_back(event);
        }
    }

    /// Get the current uptime since the supervisor started.
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Get the total restart count.
    pub async fn restart_count(&self) -> u32 {
        *self.restart_count.read().await
    }

    /// Get the last restart timestamp.
    pub async fn last_restart(&self) -> Option<DateTime<Utc>> {
        *self.last_restart.read().await
    }

    /// Get the last restart reason.
    pub async fn last_restart_reason(&self) -> Option<RestartReason> {
        self.last_restart_reason.read().await.clone()
    }

    /// Get the restart history as a vector.
    pub async fn restart_history(&self) -> Vec<RestartEvent> {
        self.restart_history.read().await.iter().cloned().collect()
    }

    /// Build a complete SupervisorStats snapshot.
    pub async fn snapshot(&self, current_health: HealthStatus) -> SupervisorStats {
        SupervisorStats {
            uptime: self.uptime(),
            restart_count: *self.restart_count.read().await,
            last_restart: *self.last_restart.read().await,
            last_restart_reason: self
                .last_restart_reason
                .read()
                .await
                .as_ref()
                .map(|r| r.to_string()),
            restart_history: self.restart_history.read().await.iter().cloned().collect(),
            current_health,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_new_tracker_empty() {
        let tracker = StatsTracker::new(10);
        assert_eq!(tracker.restart_count().await, 0);
        assert!(tracker.last_restart().await.is_none());
        assert!(tracker.last_restart_reason().await.is_none());
        assert!(tracker.restart_history().await.is_empty());
    }

    #[tokio::test]
    async fn test_record_restart_increments_count() {
        let tracker = StatsTracker::new(10);

        tracker
            .record_restart(RestartReason::InitialStart, true, 100)
            .await;
        assert_eq!(tracker.restart_count().await, 1);

        tracker
            .record_restart(
                RestartReason::Unresponsive {
                    duration_seconds: 65,
                },
                true,
                200,
            )
            .await;
        assert_eq!(tracker.restart_count().await, 2);
    }

    #[tokio::test]
    async fn test_last_restart_updated() {
        let tracker = StatsTracker::new(10);

        tracker
            .record_restart(
                RestartReason::Manual {
                    reason: "test".to_string(),
                },
                true,
                50,
            )
            .await;

        assert!(tracker.last_restart().await.is_some());
        let reason = tracker.last_restart_reason().await.unwrap();
        assert_eq!(
            reason,
            RestartReason::Manual {
                reason: "test".to_string()
            }
        );
    }

    #[tokio::test]
    async fn test_history_bounded() {
        let tracker = StatsTracker::new(3);

        for i in 0..5 {
            tracker
                .record_restart(
                    RestartReason::Unresponsive {
                        duration_seconds: i * 10 + 60,
                    },
                    true,
                    100,
                )
                .await;
        }

        let history = tracker.restart_history().await;
        assert_eq!(history.len(), 3);

        // Should contain the last 3 events (indices 2, 3, 4)
        if let RestartReason::Unresponsive { duration_seconds } = &history[0].reason {
            assert_eq!(*duration_seconds, 80); // i=2: 2*10+60=80
        } else {
            panic!("Expected Unresponsive reason");
        }
    }

    #[tokio::test]
    async fn test_uptime_increases() {
        let tracker = StatsTracker::new(10);
        let uptime1 = tracker.uptime();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let uptime2 = tracker.uptime();
        assert!(uptime2 > uptime1);
    }

    #[tokio::test]
    async fn test_snapshot() {
        let tracker = StatsTracker::new(10);
        tracker
            .record_restart(RestartReason::InitialStart, true, 50)
            .await;

        let stats = tracker.snapshot(HealthStatus::Healthy).await;
        assert_eq!(stats.restart_count, 1);
        assert!(stats.last_restart.is_some());
        assert_eq!(stats.last_restart_reason.as_deref(), Some("initial start"));
        assert_eq!(stats.restart_history.len(), 1);
        assert_eq!(stats.current_health, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_failed_restart_recorded() {
        let tracker = StatsTracker::new(10);
        tracker
            .record_restart(RestartReason::UnexpectedTermination, false, 5000)
            .await;

        let history = tracker.restart_history().await;
        assert_eq!(history.len(), 1);
        assert!(!history[0].success);
        assert_eq!(history[0].duration_ms, 5000);
    }
}
