//! Property-based tests for conversation persistence.
//!
//! **Validates: Requirements 14.1, 14.2**
//!
//! Property 26: Conversation History Persistence Round-Trip
//! Property 27: Session Context Isolation

use chrono::{DateTime, TimeZone, Utc};
use common::{Message, MessageId, MessageRole, SessionId, ToolCall, ToolResult};
use memory_store::{init_database, get_history, store_message};
use proptest::prelude::*;
use sqlx::sqlite::SqlitePool;
use tempfile::TempDir;

// ============================================================================
// Strategies for generating arbitrary messages
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
    "[a-zA-Z0-9 .,!?\\-]{1,200}"
}

/// Generate an arbitrary ToolCall.
fn arb_tool_call() -> impl Strategy<Value = ToolCall> {
    (
        "[a-z_]{3,15}",       // id
        "[a-z_]{3,20}",       // name
        "[a-zA-Z0-9]{1,30}",  // argument value
    )
        .prop_map(|(id, name, arg_val)| ToolCall {
            id: format!("call_{}", id),
            name,
            arguments: serde_json::json!({ "input": arg_val }),
        })
}

/// Generate an optional Vec of ToolCalls.
fn arb_tool_calls() -> impl Strategy<Value = Option<Vec<ToolCall>>> {
    prop_oneof![
        3 => Just(None),
        1 => proptest::collection::vec(arb_tool_call(), 1..=3).prop_map(Some),
    ]
}

/// Generate an arbitrary ToolResult.
fn arb_tool_result() -> impl Strategy<Value = ToolResult> {
    (
        "[a-z_]{3,15}",       // tool_call_id
        "[a-zA-Z0-9 ]{1,50}", // output
        any::<bool>(),         // is_error
    )
        .prop_map(|(id, output, is_error)| ToolResult {
            tool_call_id: format!("call_{}", id),
            output,
            is_error,
        })
}

/// Generate an optional Vec of ToolResults.
fn arb_tool_results() -> impl Strategy<Value = Option<Vec<ToolResult>>> {
    prop_oneof![
        3 => Just(None),
        1 => proptest::collection::vec(arb_tool_result(), 1..=3).prop_map(Some),
    ]
}

/// Generate an arbitrary token count.
fn arb_token_count() -> impl Strategy<Value = u32> {
    1..=5000u32
}

/// Generate a timestamp within a reasonable range (avoids sub-second precision
/// issues with RFC3339 round-tripping through SQLite text storage).
fn arb_timestamp() -> impl Strategy<Value = DateTime<Utc>> {
    // Generate timestamps between 2020-01-01 and 2030-01-01 at second precision
    (1577836800i64..1893456000i64).prop_map(|secs| {
        Utc.timestamp_opt(secs, 0).unwrap()
    })
}

/// Generate a complete arbitrary Message for a given session.
#[allow(dead_code)]
fn arb_message(session_id: SessionId) -> impl Strategy<Value = Message> {
    (
        arb_role(),
        arb_content(),
        arb_tool_calls(),
        arb_tool_results(),
        arb_timestamp(),
        arb_token_count(),
    )
        .prop_map(move |(role, content, tool_calls, tool_results, timestamp, token_count)| {
            Message {
                id: MessageId::new(),
                session_id,
                role,
                content,
                tool_calls,
                tool_results,
                timestamp,
                token_count,
            }
        })
}

// ============================================================================
// Test infrastructure
// ============================================================================

/// Set up a test database with a user and two sessions for FK constraints.
async fn setup_test_db() -> (SqlitePool, TempDir, SessionId, SessionId) {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = tmp_dir.path().join("prop_test_conv.db");
    let pool = init_database(&db_path).await.expect("failed to init db");

    // Create a user (FK requirement for sessions)
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, created_at)
         VALUES ('user-prop', 'propuser', 'hash', 'admin', '2024-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Create two sessions for isolation testing
    let session_a = SessionId::new();
    let session_b = SessionId::new();

    sqlx::query(
        "INSERT INTO sessions (id, user_id, mode, created_at, last_activity)
         VALUES (?, 'user-prop', 'general', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
    )
    .bind(session_a.0.to_string())
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO sessions (id, user_id, mode, created_at, last_activity)
         VALUES (?, 'user-prop', 'general', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
    )
    .bind(session_b.0.to_string())
    .execute(&pool)
    .await
    .unwrap();

    (pool, tmp_dir, session_a, session_b)
}

// ============================================================================
// Property 26: Conversation History Persistence Round-Trip
//
// For any valid message stored via store_message(), retrieving it via
// get_history() SHALL return a message with identical content, role,
// timestamp, and tool calls/results.
//
// **Validates: Requirements 14.1, 14.2**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// **Validates: Requirements 14.1, 14.2**
    ///
    /// Property 26: For any valid message stored via store_message(), retrieving
    /// it via get_history() SHALL return a message with identical content, role,
    /// timestamp, and tool calls/results.
    #[test]
    fn prop_conversation_round_trip(
        role in arb_role(),
        content in arb_content(),
        tool_calls in arb_tool_calls(),
        tool_results in arb_tool_results(),
        timestamp in arb_timestamp(),
        token_count in arb_token_count(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (pool, _tmp, session_a, _) = setup_test_db().await;

            let original = Message {
                id: MessageId::new(),
                session_id: session_a,
                role,
                content: content.clone(),
                tool_calls: tool_calls.clone(),
                tool_results: tool_results.clone(),
                timestamp,
                token_count,
            };

            // Store the message
            let stored_id = store_message(&pool, &original).await.unwrap();
            assert_eq!(stored_id, original.id);

            // Retrieve it
            let history = get_history(&pool, session_a, 100).await.unwrap();
            assert_eq!(history.len(), 1, "Expected exactly 1 message in history");

            let retrieved = &history[0];

            // Verify all fields match
            assert_eq!(retrieved.id, original.id, "MessageId mismatch");
            assert_eq!(retrieved.session_id, original.session_id, "SessionId mismatch");
            assert_eq!(retrieved.role, original.role, "Role mismatch");
            assert_eq!(retrieved.content, original.content, "Content mismatch");
            assert_eq!(retrieved.token_count, original.token_count, "Token count mismatch");
            assert_eq!(
                retrieved.timestamp.timestamp(),
                original.timestamp.timestamp(),
                "Timestamp mismatch"
            );

            // Verify tool_calls round-trip
            match (&retrieved.tool_calls, &original.tool_calls) {
                (None, None) => {}
                (Some(ret_tc), Some(orig_tc)) => {
                    assert_eq!(ret_tc.len(), orig_tc.len(), "Tool calls count mismatch");
                    for (r, o) in ret_tc.iter().zip(orig_tc.iter()) {
                        assert_eq!(r.id, o.id, "ToolCall id mismatch");
                        assert_eq!(r.name, o.name, "ToolCall name mismatch");
                        assert_eq!(r.arguments, o.arguments, "ToolCall arguments mismatch");
                    }
                }
                _ => panic!("Tool calls presence mismatch: retrieved={:?}, original={:?}",
                    retrieved.tool_calls.is_some(), original.tool_calls.is_some()),
            }

            // Verify tool_results round-trip
            match (&retrieved.tool_results, &original.tool_results) {
                (None, None) => {}
                (Some(ret_tr), Some(orig_tr)) => {
                    assert_eq!(ret_tr.len(), orig_tr.len(), "Tool results count mismatch");
                    for (r, o) in ret_tr.iter().zip(orig_tr.iter()) {
                        assert_eq!(r.tool_call_id, o.tool_call_id, "ToolResult tool_call_id mismatch");
                        assert_eq!(r.output, o.output, "ToolResult output mismatch");
                        assert_eq!(r.is_error, o.is_error, "ToolResult is_error mismatch");
                    }
                }
                _ => panic!("Tool results presence mismatch: retrieved={:?}, original={:?}",
                    retrieved.tool_results.is_some(), original.tool_results.is_some()),
            }
        });
    }
}

// ============================================================================
// Property 27: Session Context Isolation
//
// For any two distinct sessions, messages stored in one session SHALL never
// appear in the history of the other session.
//
// **Validates: Requirements 14.1, 14.2**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// **Validates: Requirements 14.1, 14.2**
    ///
    /// Property 27: For any two distinct sessions, messages stored in one session
    /// SHALL never appear in the history of the other session.
    #[test]
    fn prop_session_context_isolation(
        num_messages_a in 1..=10usize,
        num_messages_b in 1..=10usize,
        role_a in arb_role(),
        role_b in arb_role(),
        content_a in arb_content(),
        content_b in arb_content(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (pool, _tmp, session_a, session_b) = setup_test_db().await;

            let base_time = Utc.timestamp_opt(1700000000, 0).unwrap();

            // Store messages in session A
            for i in 0..num_messages_a {
                let msg = Message {
                    id: MessageId::new(),
                    session_id: session_a,
                    role: role_a,
                    content: format!("session_a_msg_{}: {}", i, content_a),
                    tool_calls: None,
                    tool_results: None,
                    timestamp: base_time + chrono::Duration::seconds(i as i64),
                    token_count: 10,
                };
                store_message(&pool, &msg).await.unwrap();
            }

            // Store messages in session B
            for i in 0..num_messages_b {
                let msg = Message {
                    id: MessageId::new(),
                    session_id: session_b,
                    role: role_b,
                    content: format!("session_b_msg_{}: {}", i, content_b),
                    tool_calls: None,
                    tool_results: None,
                    timestamp: base_time + chrono::Duration::seconds(i as i64),
                    token_count: 10,
                };
                store_message(&pool, &msg).await.unwrap();
            }

            // Retrieve history for session A
            let history_a = get_history(&pool, session_a, 1000).await.unwrap();
            // Retrieve history for session B
            let history_b = get_history(&pool, session_b, 1000).await.unwrap();

            // Verify counts
            assert_eq!(
                history_a.len(), num_messages_a,
                "Session A should have exactly {} messages, got {}",
                num_messages_a, history_a.len()
            );
            assert_eq!(
                history_b.len(), num_messages_b,
                "Session B should have exactly {} messages, got {}",
                num_messages_b, history_b.len()
            );

            // Verify session A messages all belong to session A
            for msg in &history_a {
                assert_eq!(
                    msg.session_id, session_a,
                    "Message in session A history has wrong session_id: {:?}",
                    msg.session_id
                );
                assert!(
                    msg.content.starts_with("session_a_msg_"),
                    "Message in session A history has unexpected content: {}",
                    msg.content
                );
            }

            // Verify session B messages all belong to session B
            for msg in &history_b {
                assert_eq!(
                    msg.session_id, session_b,
                    "Message in session B history has wrong session_id: {:?}",
                    msg.session_id
                );
                assert!(
                    msg.content.starts_with("session_b_msg_"),
                    "Message in session B history has unexpected content: {}",
                    msg.content
                );
            }

            // Cross-check: no message ID from session A appears in session B
            let ids_a: std::collections::HashSet<_> = history_a.iter().map(|m| m.id).collect();
            let ids_b: std::collections::HashSet<_> = history_b.iter().map(|m| m.id).collect();
            assert!(
                ids_a.is_disjoint(&ids_b),
                "Sessions A and B should have completely disjoint message IDs"
            );
        });
    }
}
