//! Property-based tests for rate limiting and brute-force protection.
//!
//! **Validates: Requirements 9.6, 9.7, 10.4**
//!
//! - Property 16: Rate Limit Enforcement
//! - Property 19: Brute-Force Lockout Enforcement

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use proptest::prelude::*;

use common::errors::SecurityError;
use common::types::ApiKeyId;
use security_layer::brute_force::{BruteForceConfig, BruteForceProtection};
use security_layer::rate_limit::{RateLimitConfig, RateLimiter};

// ============================================================================
// Strategies
// ============================================================================

/// Generate a rate limit N in the range 1..=50.
fn rate_limit_strategy() -> impl Strategy<Value = u32> {
    1u32..=50u32
}

/// Generate an arbitrary IPv4 address for brute-force testing.
fn ipv4_strategy() -> impl Strategy<Value = IpAddr> {
    (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>())
        .prop_map(|(a, b, c, d)| IpAddr::V4(Ipv4Addr::new(a, b, c, d)))
}

// ============================================================================
// Property 16: Rate Limit Enforcement
//
// For any API key with a configured limit of N requests/minute, the (N+1)th
// request within a 60-second window SHALL be rejected with RateLimitExceeded.
//
// **Validates: Requirements 9.6, 9.7**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 9.6, 9.7**
    ///
    /// Property 16: For any rate limit N in [1, 50], the first N requests SHALL
    /// be allowed, and the (N+1)th request SHALL be rejected with RateLimitExceeded.
    #[test]
    fn prop_rate_limit_rejects_request_exceeding_limit(
        limit in rate_limit_strategy()
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let config = RateLimitConfig {
                default_limit: limit,
                window: Duration::from_secs(60),
            };
            let limiter = RateLimiter::new(config);
            let key_id = ApiKeyId::new();

            // First N requests should all succeed
            for i in 0..limit {
                let result = limiter.check_rate_limit(key_id, None).await;
                prop_assert!(
                    result.is_ok(),
                    "Request {} of {} should be allowed, got {:?}",
                    i + 1,
                    limit,
                    result
                );
            }

            // The (N+1)th request should be rejected
            let result = limiter.check_rate_limit(key_id, None).await;
            prop_assert!(
                matches!(result, Err(SecurityError::RateLimitExceeded { .. })),
                "Request {} (exceeding limit of {}) should be rejected with RateLimitExceeded, got {:?}",
                limit + 1,
                limit,
                result
            );

            Ok(())
        })?;
    }

    /// **Validates: Requirements 9.6, 9.7**
    ///
    /// Property 16: For any per-key override limit N in [1, 50], the (N+1)th
    /// request SHALL be rejected regardless of the default limit.
    #[test]
    fn prop_rate_limit_per_key_override_enforced(
        limit in rate_limit_strategy()
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Set a high default limit, but override per-key to `limit`
            let config = RateLimitConfig {
                default_limit: 1000,
                window: Duration::from_secs(60),
            };
            let limiter = RateLimiter::new(config);
            let key_id = ApiKeyId::new();

            // First N requests with per-key override should succeed
            for i in 0..limit {
                let result = limiter.check_rate_limit(key_id, Some(limit)).await;
                prop_assert!(
                    result.is_ok(),
                    "Request {} of {} (per-key override) should be allowed, got {:?}",
                    i + 1,
                    limit,
                    result
                );
            }

            // The (N+1)th request should be rejected
            let result = limiter.check_rate_limit(key_id, Some(limit)).await;
            prop_assert!(
                matches!(result, Err(SecurityError::RateLimitExceeded { .. })),
                "Request {} (exceeding per-key limit of {}) should be rejected, got {:?}",
                limit + 1,
                limit,
                result
            );

            Ok(())
        })?;
    }

    /// **Validates: Requirements 9.6, 9.7**
    ///
    /// Property 16: The RateLimitExceeded error SHALL include a positive
    /// retry_after duration that does not exceed the window size.
    #[test]
    fn prop_rate_limit_retry_after_is_valid(
        limit in rate_limit_strategy()
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let config = RateLimitConfig {
                default_limit: limit,
                window: Duration::from_secs(60),
            };
            let limiter = RateLimiter::new(config);
            let key_id = ApiKeyId::new();

            // Exhaust the limit
            for _ in 0..limit {
                limiter.check_rate_limit(key_id, None).await.unwrap();
            }

            // Verify the retry_after in the error
            let result = limiter.check_rate_limit(key_id, None).await;
            match result {
                Err(SecurityError::RateLimitExceeded { retry_after }) => {
                    prop_assert!(
                        retry_after.as_secs() > 0,
                        "retry_after should be positive, got {:?}",
                        retry_after
                    );
                    prop_assert!(
                        retry_after.as_secs() <= 60,
                        "retry_after should not exceed window (60s), got {:?}",
                        retry_after
                    );
                }
                other => {
                    prop_assert!(false, "Expected RateLimitExceeded, got {:?}", other);
                }
            }

            Ok(())
        })?;
    }
}

// ============================================================================
// Property 19: Brute-Force Lockout Enforcement
//
// For any IP address that accumulates 5 consecutive failures within 10 minutes,
// the next check SHALL return IpBlocked.
//
// **Validates: Requirements 10.4**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 10.4**
    ///
    /// Property 19: For any IP address, after exactly 5 consecutive failures
    /// within the window, check_ip SHALL return IpBlocked.
    #[test]
    fn prop_brute_force_blocks_after_5_failures(
        ip in ipv4_strategy()
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let config = BruteForceConfig {
                max_failures: 5,
                failure_window: Duration::from_secs(10 * 60),
                block_duration: Duration::from_secs(15 * 60),
            };
            let protection = BruteForceProtection::new(config);

            // IP should not be blocked initially
            prop_assert!(
                protection.check_ip(ip).await.is_ok(),
                "IP {:?} should not be blocked initially",
                ip
            );

            // Record 4 failures — should still not be blocked
            for i in 0..4 {
                protection.record_failure(ip).await;
                prop_assert!(
                    protection.check_ip(ip).await.is_ok(),
                    "IP {:?} should not be blocked after {} failures",
                    ip,
                    i + 1
                );
            }

            // 5th failure triggers the block
            protection.record_failure(ip).await;

            let result = protection.check_ip(ip).await;
            prop_assert!(
                matches!(result, Err(SecurityError::IpBlocked { .. })),
                "IP {:?} should be blocked after 5 failures, got {:?}",
                ip,
                result
            );

            Ok(())
        })?;
    }

    /// **Validates: Requirements 10.4**
    ///
    /// Property 19: For any IP address, fewer than 5 consecutive failures
    /// SHALL NOT trigger a block.
    #[test]
    fn prop_brute_force_does_not_block_below_threshold(
        ip in ipv4_strategy(),
        failure_count in 1u32..5u32
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let config = BruteForceConfig {
                max_failures: 5,
                failure_window: Duration::from_secs(10 * 60),
                block_duration: Duration::from_secs(15 * 60),
            };
            let protection = BruteForceProtection::new(config);

            // Record fewer than 5 failures
            for _ in 0..failure_count {
                protection.record_failure(ip).await;
            }

            // Should NOT be blocked
            let result = protection.check_ip(ip).await;
            prop_assert!(
                result.is_ok(),
                "IP {:?} should NOT be blocked after only {} failures (threshold is 5), got {:?}",
                ip,
                failure_count,
                result
            );

            Ok(())
        })?;
    }

    /// **Validates: Requirements 10.4**
    ///
    /// Property 19: A blocked IP SHALL remain blocked regardless of subsequent
    /// successful authentication attempts (block must expire naturally).
    #[test]
    fn prop_brute_force_block_persists_despite_success(
        ip in ipv4_strategy()
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let config = BruteForceConfig {
                max_failures: 5,
                failure_window: Duration::from_secs(10 * 60),
                block_duration: Duration::from_secs(15 * 60),
            };
            let protection = BruteForceProtection::new(config);

            // Trigger the block
            for _ in 0..5 {
                protection.record_failure(ip).await;
            }

            // Verify blocked
            prop_assert!(
                protection.check_ip(ip).await.is_err(),
                "IP {:?} should be blocked after 5 failures",
                ip
            );

            // Record a success — block should persist
            protection.record_success(ip).await;

            let result = protection.check_ip(ip).await;
            prop_assert!(
                matches!(result, Err(SecurityError::IpBlocked { .. })),
                "IP {:?} should remain blocked after record_success (block must expire naturally), got {:?}",
                ip,
                result
            );

            Ok(())
        })?;
    }

    /// **Validates: Requirements 10.4**
    ///
    /// Property 19: Blocking one IP SHALL NOT affect other IPs.
    #[test]
    fn prop_brute_force_isolation_between_ips(
        ip1 in ipv4_strategy(),
        ip2 in ipv4_strategy()
    ) {
        // Skip if both IPs happen to be the same
        prop_assume!(ip1 != ip2);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let config = BruteForceConfig {
                max_failures: 5,
                failure_window: Duration::from_secs(10 * 60),
                block_duration: Duration::from_secs(15 * 60),
            };
            let protection = BruteForceProtection::new(config);

            // Block ip1
            for _ in 0..5 {
                protection.record_failure(ip1).await;
            }

            // ip1 should be blocked
            prop_assert!(
                protection.check_ip(ip1).await.is_err(),
                "IP {:?} should be blocked",
                ip1
            );

            // ip2 should NOT be blocked
            prop_assert!(
                protection.check_ip(ip2).await.is_ok(),
                "IP {:?} should NOT be blocked (only {:?} was blocked), got {:?}",
                ip2,
                ip1,
                protection.check_ip(ip2).await
            );

            Ok(())
        })?;
    }
}
