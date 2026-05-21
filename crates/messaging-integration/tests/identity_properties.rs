//! Property-based tests for messaging identity authorization.
//!
//! **Validates: Requirements 17.8, 17.9**
//!
//! Property 34: Messaging Identity Authorization
//!
//! For any incoming message from a platform user, if the user is NOT mapped to
//! an operator account, the message is rejected with an Unauthorized error.
//! If the user IS mapped, the message is authorized and the correct UserId is
//! returned. The authorization decision is consistent regardless of message
//! content, platform, or timing.

use chrono::Utc;
use common::models::Role;
use common::types::UserId;
use messaging_integration::identity::{authorize_message, IdentityMapper};
use messaging_integration::{IncomingMessage, MessagingError, Platform};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

// ============================================================================
// Strategies for generating arbitrary messaging data
// ============================================================================

/// Generate an arbitrary Platform variant.
fn arb_platform() -> impl Strategy<Value = Platform> {
    prop_oneof![
        Just(Platform::Telegram),
        Just(Platform::Discord),
        Just(Platform::WhatsApp),
    ]
}

/// Generate an arbitrary Role (both have SendMessages permission).
fn arb_role() -> impl Strategy<Value = Role> {
    prop_oneof![Just(Role::Admin), Just(Role::Operator),]
}

/// Generate an arbitrary platform user ID (non-empty string).
fn arb_platform_user_id() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_]{1,32}"
}

/// Generate arbitrary message content (any string, including empty).
fn arb_message_content() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,!?@#$%^&*()]{0,200}"
}

/// Generate an arbitrary UserId.
fn arb_user_id() -> impl Strategy<Value = UserId> {
    any::<u128>().prop_map(|n| UserId(uuid::Uuid::from_u128(n)))
}

/// Generate a set of identity mappings: Vec<(Platform, platform_user_id, UserId, Role)>.
fn arb_mappings() -> impl Strategy<Value = Vec<(Platform, String, UserId, Role)>> {
    proptest::collection::vec(
        (
            arb_platform(),
            arb_platform_user_id(),
            arb_user_id(),
            arb_role(),
        ),
        0..=20,
    )
}

/// Helper to build an IncomingMessage.
fn make_incoming(platform: Platform, platform_user_id: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        platform,
        platform_user_id: platform_user_id.to_string(),
        channel_id: None,
        content: content.to_string(),
        timestamp: Utc::now(),
    }
}

/// Helper to build an IdentityMapper from a list of mappings.
/// Since add_mapping replaces duplicates, the last mapping for a given
/// (platform, platform_user_id) pair wins.
fn build_mapper(mappings: &[(Platform, String, UserId, Role)]) -> IdentityMapper {
    let mut mapper = IdentityMapper::new();
    for (platform, platform_user_id, user_id, role) in mappings {
        mapper.add_mapping(*platform, platform_user_id.clone(), *user_id, *role);
    }
    mapper
}

// ============================================================================
// Property 34: Messaging Identity Authorization
//
// For any incoming message from a platform user:
// - If the user IS mapped to an operator account, the message is authorized
//   and the correct UserId is returned.
// - If the user is NOT mapped to an operator account, the message is rejected
//   with an Unauthorized error.
// - The authorization decision is consistent regardless of message content,
//   platform, or timing.
//
// **Validates: Requirements 17.8, 17.9**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 17.8, 17.9**
    ///
    /// Property 34a: Mapped users are always authorized and the correct UserId
    /// is returned. Since add_mapping replaces duplicates, we verify against
    /// the last mapping for each (platform, platform_user_id) pair.
    #[test]
    fn prop_mapped_users_always_authorized(
        mappings in arb_mappings(),
        content in arb_message_content(),
    ) {
        let mapper = build_mapper(&mappings);

        // Build the effective mapping (last write wins for duplicate keys)
        let mut effective: std::collections::HashMap<(Platform, String), (UserId, Role)> =
            std::collections::HashMap::new();
        for (platform, platform_user_id, user_id, role) in &mappings {
            effective.insert(
                (*platform, platform_user_id.clone()),
                (*user_id, *role),
            );
        }

        // For each effective mapping, verify authorization succeeds with correct UserId
        for ((platform, platform_user_id), (expected_user_id, _role)) in &effective {
            let incoming = make_incoming(*platform, platform_user_id, &content);
            let result = authorize_message(&mapper, &incoming);

            prop_assert!(
                result.is_ok(),
                "Mapped user ({:?}, '{}') should be authorized, got: {:?}",
                platform, platform_user_id, result
            );

            let returned_user_id = result.unwrap();
            prop_assert_eq!(
                returned_user_id, *expected_user_id,
                "Returned UserId should match the mapped UserId for ({:?}, '{}')",
                platform, platform_user_id
            );
        }
    }

    /// **Validates: Requirements 17.8, 17.9**
    ///
    /// Property 34b: Unmapped users are always rejected with an Unauthorized error.
    #[test]
    fn prop_unmapped_users_always_rejected(
        mappings in arb_mappings(),
        unmapped_platform in arb_platform(),
        unmapped_user_id in arb_platform_user_id(),
        content in arb_message_content(),
    ) {
        let mapper = build_mapper(&mappings);

        // Check if this user is actually unmapped (skip if it happens to collide)
        let is_mapped = mappings.iter().any(|(p, uid, _, _)| {
            *p == unmapped_platform && *uid == unmapped_user_id
        });

        if !is_mapped {
            let incoming = make_incoming(unmapped_platform, &unmapped_user_id, &content);
            let result = authorize_message(&mapper, &incoming);

            prop_assert!(
                result.is_err(),
                "Unmapped user ({:?}, '{}') should be rejected, got: {:?}",
                unmapped_platform, unmapped_user_id, result
            );

            // Verify it's specifically an Unauthorized error
            match result.unwrap_err() {
                MessagingError::Unauthorized(_) => {} // expected
                other => {
                    return Err(TestCaseError::Fail(
                        format!(
                            "Expected Unauthorized error for unmapped user, got: {:?}",
                            other
                        ).into()
                    ));
                }
            }
        }
    }

    /// **Validates: Requirements 17.8, 17.9**
    ///
    /// Property 34c: The authorization decision is independent of message content.
    /// For the same user identity, different message contents produce the same
    /// authorization result.
    #[test]
    fn prop_auth_independent_of_message_content(
        platform in arb_platform(),
        platform_user_id in arb_platform_user_id(),
        user_id in arb_user_id(),
        role in arb_role(),
        content_a in arb_message_content(),
        content_b in arb_message_content(),
        is_mapped in any::<bool>(),
    ) {
        let mut mapper = IdentityMapper::new();
        if is_mapped {
            mapper.add_mapping(platform, platform_user_id.clone(), user_id, role);
        }

        let incoming_a = make_incoming(platform, &platform_user_id, &content_a);
        let incoming_b = make_incoming(platform, &platform_user_id, &content_b);

        let result_a = authorize_message(&mapper, &incoming_a);
        let result_b = authorize_message(&mapper, &incoming_b);

        // Both should have the same authorization outcome
        prop_assert_eq!(
            result_a.is_ok(), result_b.is_ok(),
            "Authorization decision should be the same regardless of content. \
             Content A: '{}', Content B: '{}', Result A: {:?}, Result B: {:?}",
            content_a, content_b, result_a, result_b
        );

        // If both authorized, they should return the same UserId
        if let (Ok(uid_a), Ok(uid_b)) = (&result_a, &result_b) {
            prop_assert_eq!(
                uid_a, uid_b,
                "Same user should get same UserId regardless of message content"
            );
        }
    }

    /// **Validates: Requirements 17.8, 17.9**
    ///
    /// Property 34d: The authorization decision is consistent across multiple calls.
    /// Calling authorize_message multiple times for the same user produces the same
    /// result (no state mutation affecting auth).
    #[test]
    fn prop_auth_consistent_across_multiple_calls(
        mappings in arb_mappings(),
        test_platform in arb_platform(),
        test_user_id in arb_platform_user_id(),
        content in arb_message_content(),
        num_calls in 2..=10usize,
    ) {
        let mapper = build_mapper(&mappings);

        let incoming = make_incoming(test_platform, &test_user_id, &content);

        // Call authorize_message multiple times
        let first_result = authorize_message(&mapper, &incoming);
        let first_is_ok = first_result.is_ok();
        let first_user_id = first_result.ok();

        for call_num in 1..num_calls {
            let result = authorize_message(&mapper, &incoming);
            prop_assert_eq!(
                result.is_ok(), first_is_ok,
                "Call {} should have same auth outcome as call 0", call_num
            );

            if let (Some(ref expected), Ok(actual)) = (&first_user_id, &result) {
                prop_assert_eq!(
                    expected, actual,
                    "Call {} should return same UserId as call 0", call_num
                );
            }
        }
    }
}
