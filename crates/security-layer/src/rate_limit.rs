//! Per-API-key rate limiting for the VPS AI Agent Platform.
//!
//! Implements a sliding window rate limiter that tracks request counts
//! per API key. Each key has a configurable maximum requests-per-minute
//! limit (default: 100).
//!
//! When the limit is exceeded, returns `SecurityError::RateLimitExceeded`
//! with a `retry_after` duration indicating when the client can retry.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::warn;

use common::errors::SecurityError;
use common::types::ApiKeyId;

/// Default rate limit: 100 requests per minute.
pub const DEFAULT_RATE_LIMIT: u32 = 100;

/// The sliding window size for rate limiting.
const WINDOW_SIZE: Duration = Duration::from_secs(60);

/// Configuration for rate limiting behavior.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Default maximum requests per minute if not specified per-key.
    pub default_limit: u32,
    /// The time window over which requests are counted.
    pub window: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            default_limit: DEFAULT_RATE_LIMIT,
            window: WINDOW_SIZE,
        }
    }
}

/// Record of requests made with a single API key within the sliding window.
#[derive(Debug, Clone)]
struct RequestRecord {
    /// Timestamps of requests within the current window.
    timestamps: Vec<DateTime<Utc>>,
}

/// Per-API-key sliding window rate limiter.
///
/// Thread-safe, designed for concurrent access from multiple request handlers.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    config: RateLimitConfig,
    /// Mutable default limit — overrides `config.default_limit` when set.
    /// Wrapped in an Arc so clones share the same atomic; updating via
    /// `set_default_limit` affects every handle.
    current_default_limit: Arc<AtomicU32>,
    records: Arc<RwLock<HashMap<ApiKeyId, RequestRecord>>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        let initial = config.default_limit;
        Self {
            config,
            current_default_limit: Arc::new(AtomicU32::new(initial)),
            records: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Replace the default per-key limit at runtime. Returns the previous value.
    /// Affects every clone of this limiter (atomic is shared via Arc).
    pub fn set_default_limit(&self, new_limit: u32) -> u32 {
        let new_limit = new_limit.max(1);
        self.current_default_limit
            .swap(new_limit, Ordering::Relaxed)
    }

    /// Get the live default limit (may differ from `config.default_limit` if
    /// `set_default_limit` has been called).
    pub fn default_limit(&self) -> u32 {
        self.current_default_limit.load(Ordering::Relaxed)
    }

    /// Check whether a request from the given API key is allowed.
    ///
    /// If allowed, records the request and returns `Ok(())`.
    /// If the rate limit is exceeded, returns `Err(SecurityError::RateLimitExceeded)`
    /// with a `retry_after` duration.
    ///
    /// The `key_limit` parameter allows per-key overrides of the default limit.
    /// Pass `None` to use the configured default.
    pub async fn check_rate_limit(
        &self,
        key_id: ApiKeyId,
        key_limit: Option<u32>,
    ) -> Result<(), SecurityError> {
        let now = Utc::now();
        let limit = key_limit.unwrap_or_else(|| self.current_default_limit.load(Ordering::Relaxed));
        let window =
            chrono::Duration::from_std(self.config.window).unwrap_or(chrono::Duration::seconds(60));
        let window_start = now - window;

        let mut records = self.records.write().await;
        let record = records.entry(key_id).or_insert_with(|| RequestRecord {
            timestamps: Vec::new(),
        });

        // Remove timestamps outside the sliding window
        record.timestamps.retain(|t| *t >= window_start);

        // Check if limit is exceeded
        if record.timestamps.len() >= limit as usize {
            // Calculate retry_after: time until the oldest request in the window expires
            let oldest_in_window = record.timestamps.first().copied().unwrap_or(now);
            let expires_at = oldest_in_window + window;
            let retry_after = if expires_at > now {
                (expires_at - now)
                    .to_std()
                    .unwrap_or(Duration::from_secs(1))
            } else {
                Duration::from_secs(1)
            };

            warn!(
                key_id = %key_id,
                limit = limit,
                retry_after_secs = retry_after.as_secs(),
                "Rate limit exceeded for API key"
            );

            return Err(SecurityError::RateLimitExceeded { retry_after });
        }

        // Record this request
        record.timestamps.push(now);
        Ok(())
    }

    /// Remove expired records to free memory.
    ///
    /// Should be called periodically to prevent unbounded memory growth.
    pub async fn cleanup_expired(&self) {
        let now = Utc::now();
        let window =
            chrono::Duration::from_std(self.config.window).unwrap_or(chrono::Duration::seconds(60));
        let window_start = now - window;

        let mut records = self.records.write().await;

        // First clean up timestamps within each record
        for record in records.values_mut() {
            record.timestamps.retain(|t| *t >= window_start);
        }

        // Then remove empty records
        records.retain(|_, record| !record.timestamps.is_empty());
    }

    /// Get the number of tracked API keys (for monitoring/metrics).
    pub async fn tracked_key_count(&self) -> usize {
        self.records.read().await.len()
    }

    /// Get the current request count for a specific API key within the window.
    pub async fn current_count(&self, key_id: ApiKeyId) -> u32 {
        let now = Utc::now();
        let window =
            chrono::Duration::from_std(self.config.window).unwrap_or(chrono::Duration::seconds(60));
        let window_start = now - window;

        let records = self.records.read().await;
        records
            .get(&key_id)
            .map(|record| {
                record
                    .timestamps
                    .iter()
                    .filter(|t| **t >= window_start)
                    .count() as u32
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key_id() -> ApiKeyId {
        ApiKeyId::new()
    }

    #[tokio::test]
    async fn test_first_request_is_allowed() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        let key_id = test_key_id();

        assert!(limiter.check_rate_limit(key_id, None).await.is_ok());
    }

    #[tokio::test]
    async fn test_requests_within_limit_are_allowed() {
        let config = RateLimitConfig {
            default_limit: 5,
            window: Duration::from_secs(60),
        };
        let limiter = RateLimiter::new(config);
        let key_id = test_key_id();

        for _ in 0..5 {
            assert!(limiter.check_rate_limit(key_id, None).await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_exceeding_limit_returns_error() {
        let config = RateLimitConfig {
            default_limit: 3,
            window: Duration::from_secs(60),
        };
        let limiter = RateLimiter::new(config);
        let key_id = test_key_id();

        // Use up the limit
        for _ in 0..3 {
            assert!(limiter.check_rate_limit(key_id, None).await.is_ok());
        }

        // Next request should be rejected
        let result = limiter.check_rate_limit(key_id, None).await;
        assert!(matches!(
            result,
            Err(SecurityError::RateLimitExceeded { .. })
        ));
    }

    #[tokio::test]
    async fn test_rate_limit_returns_retry_after() {
        let config = RateLimitConfig {
            default_limit: 2,
            window: Duration::from_secs(60),
        };
        let limiter = RateLimiter::new(config);
        let key_id = test_key_id();

        limiter.check_rate_limit(key_id, None).await.unwrap();
        limiter.check_rate_limit(key_id, None).await.unwrap();

        let result = limiter.check_rate_limit(key_id, None).await;
        match result {
            Err(SecurityError::RateLimitExceeded { retry_after }) => {
                // retry_after should be positive and at most the window size
                assert!(retry_after.as_secs() > 0);
                assert!(retry_after.as_secs() <= 60);
            }
            _ => panic!("Expected RateLimitExceeded error"),
        }
    }

    #[tokio::test]
    async fn test_different_keys_have_independent_limits() {
        let config = RateLimitConfig {
            default_limit: 2,
            window: Duration::from_secs(60),
        };
        let limiter = RateLimiter::new(config);
        let key1 = test_key_id();
        let key2 = test_key_id();

        // Exhaust key1's limit
        limiter.check_rate_limit(key1, None).await.unwrap();
        limiter.check_rate_limit(key1, None).await.unwrap();
        assert!(limiter.check_rate_limit(key1, None).await.is_err());

        // key2 should still be allowed
        assert!(limiter.check_rate_limit(key2, None).await.is_ok());
    }

    #[tokio::test]
    async fn test_per_key_limit_override() {
        let config = RateLimitConfig {
            default_limit: 10,
            window: Duration::from_secs(60),
        };
        let limiter = RateLimiter::new(config);
        let key_id = test_key_id();

        // Override with a limit of 2
        limiter.check_rate_limit(key_id, Some(2)).await.unwrap();
        limiter.check_rate_limit(key_id, Some(2)).await.unwrap();

        // Should be rejected at the overridden limit
        let result = limiter.check_rate_limit(key_id, Some(2)).await;
        assert!(matches!(
            result,
            Err(SecurityError::RateLimitExceeded { .. })
        ));
    }

    #[tokio::test]
    async fn test_window_expiry_allows_new_requests() {
        let config = RateLimitConfig {
            default_limit: 2,
            // Very short window for testing
            window: Duration::from_millis(50),
        };
        let limiter = RateLimiter::new(config);
        let key_id = test_key_id();

        // Exhaust the limit
        limiter.check_rate_limit(key_id, None).await.unwrap();
        limiter.check_rate_limit(key_id, None).await.unwrap();
        assert!(limiter.check_rate_limit(key_id, None).await.is_err());

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should be allowed again
        assert!(limiter.check_rate_limit(key_id, None).await.is_ok());
    }

    #[tokio::test]
    async fn test_current_count() {
        let config = RateLimitConfig {
            default_limit: 10,
            window: Duration::from_secs(60),
        };
        let limiter = RateLimiter::new(config);
        let key_id = test_key_id();

        assert_eq!(limiter.current_count(key_id).await, 0);

        limiter.check_rate_limit(key_id, None).await.unwrap();
        limiter.check_rate_limit(key_id, None).await.unwrap();
        limiter.check_rate_limit(key_id, None).await.unwrap();

        assert_eq!(limiter.current_count(key_id).await, 3);
    }

    #[tokio::test]
    async fn test_cleanup_removes_expired_records() {
        let config = RateLimitConfig {
            default_limit: 10,
            window: Duration::from_millis(50),
        };
        let limiter = RateLimiter::new(config);
        let key_id = test_key_id();

        limiter.check_rate_limit(key_id, None).await.unwrap();
        assert_eq!(limiter.tracked_key_count().await, 1);

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        limiter.cleanup_expired().await;
        assert_eq!(limiter.tracked_key_count().await, 0);
    }
}
