//! Brute-force protection for the VPS AI Agent Platform.
//!
//! Tracks failed authentication attempts per IP address and blocks IPs
//! that exceed the threshold within a configurable time window.
//!
//! Default policy:
//! - 5 consecutive failures within a 10-minute window triggers a block
//! - Blocked IPs are locked out for 15 minutes

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use common::errors::SecurityError;

/// Configuration for brute-force protection thresholds.
#[derive(Debug, Clone)]
pub struct BruteForceConfig {
    /// Maximum consecutive failures before blocking (default: 5).
    pub max_failures: u32,
    /// Time window in which failures are counted (default: 10 minutes).
    pub failure_window: Duration,
    /// Duration an IP is blocked after exceeding the threshold (default: 15 minutes).
    pub block_duration: Duration,
}

impl Default for BruteForceConfig {
    fn default() -> Self {
        Self {
            max_failures: 5,
            failure_window: Duration::from_secs(10 * 60),
            block_duration: Duration::from_secs(15 * 60),
        }
    }
}

/// Record of failed authentication attempts from a single IP.
#[derive(Debug, Clone)]
struct FailureRecord {
    /// Timestamps of consecutive failed attempts within the window.
    attempts: Vec<DateTime<Utc>>,
    /// If set, the IP is blocked until this time.
    blocked_until: Option<DateTime<Utc>>,
}

/// Brute-force protection tracker.
///
/// Thread-safe, designed for concurrent access from multiple request handlers.
#[derive(Debug, Clone)]
pub struct BruteForceProtection {
    config: BruteForceConfig,
    records: Arc<RwLock<HashMap<IpAddr, FailureRecord>>>,
}

impl BruteForceProtection {
    /// Create a new brute-force protection instance with the given configuration.
    pub fn new(config: BruteForceConfig) -> Self {
        Self {
            config,
            records: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Check whether an IP address is currently blocked.
    ///
    /// Returns `Ok(())` if the IP is allowed, or `Err(SecurityError::IpBlocked)`
    /// if the IP is currently in a block period.
    pub async fn check_ip(&self, ip: IpAddr) -> Result<(), SecurityError> {
        let now = Utc::now();
        let records = self.records.read().await;

        if let Some(record) = records.get(&ip) {
            if let Some(blocked_until) = record.blocked_until {
                if now < blocked_until {
                    return Err(SecurityError::IpBlocked {
                        until: blocked_until,
                    });
                }
            }
        }

        Ok(())
    }

    /// Record a failed authentication attempt from an IP address.
    ///
    /// If the failure count exceeds the threshold within the configured window,
    /// the IP will be blocked for the configured duration.
    pub async fn record_failure(&self, ip: IpAddr) {
        let now = Utc::now();
        let mut records = self.records.write().await;

        let record = records.entry(ip).or_insert_with(|| FailureRecord {
            attempts: Vec::new(),
            blocked_until: None,
        });

        // If already blocked, don't accumulate more attempts
        if let Some(blocked_until) = record.blocked_until {
            if now < blocked_until {
                return;
            }
            // Block expired — reset the record
            record.blocked_until = None;
            record.attempts.clear();
        }

        // Add the new failure timestamp
        record.attempts.push(now);

        // Remove attempts outside the failure window
        let window_start = now
            - chrono::Duration::from_std(self.config.failure_window)
                .unwrap_or(chrono::Duration::seconds(600));
        record.attempts.retain(|t| *t >= window_start);

        // Check if threshold is exceeded
        if record.attempts.len() >= self.config.max_failures as usize {
            let blocked_until = now
                + chrono::Duration::from_std(self.config.block_duration)
                    .unwrap_or(chrono::Duration::seconds(900));
            record.blocked_until = Some(blocked_until);
            record.attempts.clear();

            warn!(
                ip = %ip,
                blocked_until = %blocked_until,
                "IP blocked due to {} consecutive authentication failures",
                self.config.max_failures
            );
        }
    }

    /// Record a successful authentication from an IP address.
    ///
    /// Clears any accumulated failure count for the IP (but does not
    /// lift an active block — the block must expire naturally).
    pub async fn record_success(&self, ip: IpAddr) {
        let mut records = self.records.write().await;

        if let Some(record) = records.get_mut(&ip) {
            // Only clear attempts if not currently blocked
            if record.blocked_until.is_none() {
                record.attempts.clear();
            }
        }
    }

    /// Remove expired blocks and stale records to free memory.
    ///
    /// Should be called periodically (e.g., every few minutes) to prevent
    /// unbounded memory growth.
    pub async fn cleanup_expired(&self) {
        let now = Utc::now();
        let mut records = self.records.write().await;

        records.retain(|ip, record| {
            // Remove records with expired blocks and no recent attempts
            if let Some(blocked_until) = record.blocked_until {
                if now >= blocked_until {
                    info!(ip = %ip, "Removing expired block record");
                    return false;
                }
            }

            // Remove records with no attempts and no block
            if record.attempts.is_empty() && record.blocked_until.is_none() {
                return false;
            }

            true
        });
    }

    /// Get the number of tracked IPs (for monitoring/metrics).
    pub async fn tracked_ip_count(&self) -> usize {
        self.records.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100))
    }

    fn other_ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
    }

    #[tokio::test]
    async fn test_new_ip_is_not_blocked() {
        let protection = BruteForceProtection::new(BruteForceConfig::default());
        assert!(protection.check_ip(test_ip()).await.is_ok());
    }

    #[tokio::test]
    async fn test_fewer_than_threshold_failures_not_blocked() {
        let protection = BruteForceProtection::new(BruteForceConfig::default());
        let ip = test_ip();

        // Record 4 failures (threshold is 5)
        for _ in 0..4 {
            protection.record_failure(ip).await;
        }

        assert!(protection.check_ip(ip).await.is_ok());
    }

    #[tokio::test]
    async fn test_threshold_failures_triggers_block() {
        let protection = BruteForceProtection::new(BruteForceConfig::default());
        let ip = test_ip();

        // Record 5 failures (threshold is 5)
        for _ in 0..5 {
            protection.record_failure(ip).await;
        }

        let result = protection.check_ip(ip).await;
        assert!(matches!(result, Err(SecurityError::IpBlocked { .. })));
    }

    #[tokio::test]
    async fn test_block_does_not_affect_other_ips() {
        let protection = BruteForceProtection::new(BruteForceConfig::default());
        let blocked_ip = test_ip();
        let clean_ip = other_ip();

        for _ in 0..5 {
            protection.record_failure(blocked_ip).await;
        }

        assert!(protection.check_ip(clean_ip).await.is_ok());
    }

    #[tokio::test]
    async fn test_success_clears_failure_count() {
        let protection = BruteForceProtection::new(BruteForceConfig::default());
        let ip = test_ip();

        // Record 4 failures
        for _ in 0..4 {
            protection.record_failure(ip).await;
        }

        // Successful auth resets the counter
        protection.record_success(ip).await;

        // Record 4 more failures — should still not be blocked
        for _ in 0..4 {
            protection.record_failure(ip).await;
        }

        assert!(protection.check_ip(ip).await.is_ok());
    }

    #[tokio::test]
    async fn test_block_expires_after_duration() {
        let config = BruteForceConfig {
            max_failures: 2,
            failure_window: Duration::from_secs(60),
            // Use a very short block for testing
            block_duration: Duration::from_millis(50),
        };
        let protection = BruteForceProtection::new(config);
        let ip = test_ip();

        // Trigger block
        protection.record_failure(ip).await;
        protection.record_failure(ip).await;

        assert!(protection.check_ip(ip).await.is_err());

        // Wait for block to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(protection.check_ip(ip).await.is_ok());
    }

    #[tokio::test]
    async fn test_cleanup_removes_expired_blocks() {
        let config = BruteForceConfig {
            max_failures: 2,
            failure_window: Duration::from_secs(60),
            block_duration: Duration::from_millis(50),
        };
        let protection = BruteForceProtection::new(config);
        let ip = test_ip();

        protection.record_failure(ip).await;
        protection.record_failure(ip).await;

        assert_eq!(protection.tracked_ip_count().await, 1);

        // Wait for block to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        protection.cleanup_expired().await;
        assert_eq!(protection.tracked_ip_count().await, 0);
    }

    #[tokio::test]
    async fn test_custom_config() {
        let config = BruteForceConfig {
            max_failures: 3,
            failure_window: Duration::from_secs(300),
            block_duration: Duration::from_secs(600),
        };
        let protection = BruteForceProtection::new(config);
        let ip = test_ip();

        // 2 failures should not block
        protection.record_failure(ip).await;
        protection.record_failure(ip).await;
        assert!(protection.check_ip(ip).await.is_ok());

        // 3rd failure triggers block
        protection.record_failure(ip).await;
        assert!(protection.check_ip(ip).await.is_err());
    }
}
