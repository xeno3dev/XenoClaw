//! Property-based tests for context summarization.
//!
//! **Validates: Requirements 14.4**
//!
//! Property 28: Context Summarization Preserves Critical Content
//!
//! When the context window token usage exceeds 80% of the configured LLM's
//! token limit, the Session_Manager SHALL summarize older context while
//! preserving all explicitly stored knowledge and the most recent 10
//! conversation turns in full.

use agent_core::{KnowledgeEntry, SessionManager, SessionManagerConfig};
use chrono::Utc;
use common::models::{Message, MessageRole};
use common::types::{MessageId, SessionId};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

// ============================================================================
// Strategies for generating arbitrary session data
// ============================================================================

/// Generate an arbitrary MessageRole.
fn arb_role() -> impl Strategy<Value = MessageRole> {
    prop_oneof![
        Just(MessageRole::User),
        Just(MessageRole::Assistant),
        Just(MessageRole::System),
        Just(MessageRole::Tool),
    ]
}

/// Generate arbitrary message content (non-empty, reasonable length).
fn arb_content() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,!?]{1,100}"
}

/// Generate a token limit for the LLM (realistic range).
fn arb_token_limit() -> impl Strategy<Value = u32> {
    1000..=200_000u32
}

/// Generate the number of messages in a session (enough to potentially trigger summarization).
fn arb_message_count() -> impl Strategy<Value = usize> {
    12..=50usize
}

/// Helper to create a message with given parameters.
fn make_message(session_id: SessionId, role: MessageRole, content: &str, tokens: u32) -> Message {
    Message {
        id: MessageId::new(),
        session_id,
        role,
        content: content.to_string(),
        tool_calls: None,
        tool_results: None,
        timestamp: Utc::now(),
        token_count: tokens,
    }
}

// ============================================================================
// Property 28: Context Summarization Preserves Critical Content
//
// When the context window token usage exceeds 80% of the configured LLM's
// token limit, the Session_Manager SHALL summarize older context while
// preserving all explicitly stored knowledge and the most recent 10
// conversation turns in full.
//
// Sub-properties tested:
// 1. After summarization, the last 10 turns are always preserved in full
// 2. All stored knowledge entries are preserved after summarization
// 3. Summarization only triggers when token usage exceeds 80% of the limit
// 4. Token count is reduced after summarization
//
// **Validates: Requirements 14.4**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// **Validates: Requirements 14.4**
    ///
    /// Property 28a: After summarization triggers, the last 10 conversation
    /// turns are always preserved in full (content unchanged).
    #[test]
    fn prop_summarization_preserves_last_10_turns(
        token_limit in arb_token_limit(),
        num_messages in arb_message_count(),
        roles in proptest::collection::vec(arb_role(), 50),
        contents in proptest::collection::vec(arb_content(), 50),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            // Calculate per-message tokens to ensure we exceed 80% threshold.
            let threshold = (token_limit as f64 * 0.80) as u32;
            let per_msg_tokens = (threshold / num_messages as u32) + 1;

            let config = SessionManagerConfig {
                token_limit,
                max_sessions: 50,
            };
            let mgr = SessionManager::new(config);
            let sid = SessionId::new();

            // Track the content of each message we add
            let mut added_contents: Vec<String> = Vec::new();

            for i in 0..num_messages {
                let role = roles[i % roles.len()];
                let content = format!("msg_{}: {}", i, &contents[i % contents.len()]);
                added_contents.push(content.clone());

                let msg = make_message(sid, role, &content, per_msg_tokens);
                mgr.add_message(sid, msg).await.unwrap();
            }

            let ctx = mgr.get_context(sid).await.unwrap();

            // Since we exceeded the threshold and have more than 10 messages,
            // summarization should have occurred.
            prop_assert!(ctx.has_summary(),
                "Summarization should have triggered: total_tokens={} > threshold={}",
                num_messages as u32 * per_msg_tokens, threshold);

            // The last 10 messages should be preserved in full.
            // After summarization, context has: [summary_msg, preserved_1, ..., preserved_10]
            let messages = ctx.messages();
            let preserved_count = messages.len() - 1; // subtract summary message
            prop_assert!(preserved_count >= 10,
                "Should preserve at least 10 turns, got {}", preserved_count);

            // Verify the last 10 added messages are preserved with original content
            let last_10_contents: Vec<&str> = added_contents[num_messages - 10..]
                .iter()
                .map(|s| s.as_str())
                .collect();

            let preserved_messages = &messages[messages.len() - 10..];
            for (i, msg) in preserved_messages.iter().enumerate() {
                prop_assert_eq!(
                    &msg.content, last_10_contents[i],
                    "Preserved message {} content mismatch", i
                );
            }

            Ok(())
        });
        result?;
    }

    /// **Validates: Requirements 14.4**
    ///
    /// Property 28b: All stored knowledge entries are preserved after
    /// summarization occurs.
    #[test]
    fn prop_summarization_preserves_all_knowledge(
        token_limit in 500..=50_000u32,
        num_messages in 15..=40usize,
        num_knowledge in 1..=10usize,
        knowledge_titles in proptest::collection::vec("[a-zA-Z ]{3,30}", 10),
        knowledge_contents in proptest::collection::vec("[a-zA-Z0-9 .,]{10,200}", 10),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let threshold = (token_limit as f64 * 0.80) as u32;
            let per_msg_tokens = (threshold / num_messages as u32) + 1;

            let config = SessionManagerConfig {
                token_limit,
                max_sessions: 50,
            };
            let mgr = SessionManager::new(config);
            let sid = SessionId::new();

            // Create the session first
            mgr.get_context(sid).await.unwrap();

            // Add knowledge entries before messages
            let mut expected_knowledge: Vec<(String, String)> = Vec::new();
            for i in 0..num_knowledge {
                let title = knowledge_titles[i % knowledge_titles.len()].clone();
                let content = knowledge_contents[i % knowledge_contents.len()].clone();
                expected_knowledge.push((title.clone(), content.clone()));

                mgr.add_knowledge(sid, KnowledgeEntry {
                    title,
                    content,
                }).await.unwrap();
            }

            // Add enough messages to trigger summarization
            for i in 0..num_messages {
                let msg = make_message(
                    sid,
                    MessageRole::User,
                    &format!("knowledge_test_msg_{}", i),
                    per_msg_tokens,
                );
                mgr.add_message(sid, msg).await.unwrap();
            }

            let ctx = mgr.get_context(sid).await.unwrap();

            // Verify summarization occurred
            prop_assert!(ctx.has_summary(),
                "Summarization should have triggered");

            // Verify ALL knowledge entries are preserved
            let knowledge = ctx.knowledge();
            prop_assert_eq!(knowledge.len(), num_knowledge,
                "All {} knowledge entries should be preserved, got {}",
                num_knowledge, knowledge.len());

            for (i, (expected_title, expected_content)) in expected_knowledge.iter().enumerate() {
                prop_assert_eq!(&knowledge[i].title, expected_title,
                    "Knowledge entry {} title mismatch", i);
                prop_assert_eq!(&knowledge[i].content, expected_content,
                    "Knowledge entry {} content mismatch", i);
            }

            Ok(())
        });
        result?;
    }

    /// **Validates: Requirements 14.4**
    ///
    /// Property 28c: Summarization only triggers when token usage exceeds
    /// 80% of the configured LLM token limit. Below the threshold, all
    /// messages remain intact.
    #[test]
    fn prop_no_summarization_below_threshold(
        token_limit in 1000..=200_000u32,
        num_messages in 11..=30usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let threshold = (token_limit as f64 * 0.80) as u32;
            // Set per-message tokens so total stays BELOW threshold
            let per_msg_tokens = if threshold / num_messages as u32 > 1 {
                (threshold / num_messages as u32) - 1
            } else {
                1
            };

            // Verify our setup: total tokens should be below threshold
            let total_tokens = num_messages as u32 * per_msg_tokens;
            prop_assume!(total_tokens <= threshold);

            let config = SessionManagerConfig {
                token_limit,
                max_sessions: 50,
            };
            let mgr = SessionManager::new(config);
            let sid = SessionId::new();

            for i in 0..num_messages {
                let msg = make_message(
                    sid,
                    MessageRole::User,
                    &format!("below_threshold_msg_{}", i),
                    per_msg_tokens,
                );
                mgr.add_message(sid, msg).await.unwrap();
            }

            let ctx = mgr.get_context(sid).await.unwrap();

            // No summarization should have occurred
            prop_assert!(!ctx.has_summary(),
                "Summarization should NOT trigger when tokens ({}) <= threshold ({})",
                total_tokens, threshold);

            // All messages should be intact
            prop_assert_eq!(ctx.messages().len(), num_messages,
                "All {} messages should be present, got {}",
                num_messages, ctx.messages().len());

            // Verify each message content is preserved
            for (i, msg) in ctx.messages().iter().enumerate() {
                let expected = format!("below_threshold_msg_{}", i);
                prop_assert_eq!(
                    &msg.content,
                    &expected,
                    "Message {} content should be unchanged", i
                );
            }

            Ok(())
        });
        result?;
    }

    /// **Validates: Requirements 14.4**
    ///
    /// Property 28d: Token count is reduced after summarization. The new
    /// token count should be less than the original total (since a summary
    /// of N messages is shorter than the N messages themselves).
    #[test]
    fn prop_token_count_reduced_after_summarization(
        token_limit in 500..=50_000u32,
        num_messages in 15..=40usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let threshold = (token_limit as f64 * 0.80) as u32;
            let per_msg_tokens = (threshold / num_messages as u32) + 1;
            let original_total = num_messages as u32 * per_msg_tokens;

            let config = SessionManagerConfig {
                token_limit,
                max_sessions: 50,
            };
            let mgr = SessionManager::new(config);
            let sid = SessionId::new();

            for i in 0..num_messages {
                let msg = make_message(
                    sid,
                    MessageRole::User,
                    &format!("reduce_test_msg_{}", i),
                    per_msg_tokens,
                );
                mgr.add_message(sid, msg).await.unwrap();
            }

            let ctx = mgr.get_context(sid).await.unwrap();

            // Verify summarization occurred
            prop_assert!(ctx.has_summary(),
                "Summarization should have triggered");

            let new_token_count = ctx.token_count();

            // Token count should be reduced from the original total
            prop_assert!(new_token_count < original_total,
                "Token count after summarization ({}) should be less than original ({})",
                new_token_count, original_total);

            // The preserved messages' tokens should be accounted for
            let preserved_tokens: u32 = 10 * per_msg_tokens;
            prop_assert!(new_token_count >= preserved_tokens,
                "Token count ({}) should be at least the preserved messages' tokens ({})",
                new_token_count, preserved_tokens);

            Ok(())
        });
        result?;
    }
}
