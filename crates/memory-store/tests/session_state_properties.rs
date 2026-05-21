//! Property-based tests for session state persistence round-trip.
//!
//! **Validates: Requirements 2.3**
//!
//! Property 3: Session State Persistence Round-Trip
//!
//! For any valid session state (including active task queue, conversation history,
//! and scheduler configuration), persisting the state and then restoring it SHALL
//! produce a state equivalent to the original.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use proptest::prelude::*;
use sqlx::sqlite::SqlitePool;
use tempfile::TempDir;
use uuid::Uuid;

use common::config::SchedulerConfig;
use common::types::{SessionId, TaskId};
use memory_store::db::init_database;
use memory_store::{
    create_session_state_table, restore_session_state, save_session_state, SessionState,
};

// ============================================================================
// Strategies for generating arbitrary SessionState values
// ============================================================================

/// Generate an arbitrary SessionId.
fn arb_session_id() -> impl Strategy<Value = SessionId> {
    prop::array::uniform16(any::<u8>()).prop_map(|bytes| SessionId(Uuid::from_bytes(bytes)))
}

/// Generate an arbitrary TaskId.
fn arb_task_id() -> impl Strategy<Value = TaskId> {
    prop::array::uniform16(any::<u8>()).prop_map(|bytes| TaskId(Uuid::from_bytes(bytes)))
}

/// Generate an arbitrary task queue of varying lengths (0 to 50 tasks).
fn arb_task_queue() -> impl Strategy<Value = Vec<TaskId>> {
    prop::collection::vec(arb_task_id(), 0..50)
}

/// Generate an arbitrary conversation context string.
/// Includes empty strings, short strings, and longer multi-line strings
/// with various characters including unicode.
fn arb_conversation_context() -> impl Strategy<Value = String> {
    prop_oneof![
        // Empty context
        Just(String::new()),
        // Simple ASCII strings
        "[a-zA-Z0-9 .,!?;:'\"-]{0,200}",
        // Strings with newlines and special chars
        "([a-zA-Z0-9 .,!?;:'\"\n\t\\-]){0,500}",
        // Unicode strings
        "\\PC{0,100}",
    ]
}

/// Generate an arbitrary SchedulerConfig with valid values.
fn arb_scheduler_config() -> impl Strategy<Value = SchedulerConfig> {
    (1..=255u8, 1..=3600u32, 1..=255u8).prop_map(
        |(max_concurrent_tasks, default_timeout_seconds, max_dependency_depth)| SchedulerConfig {
            max_concurrent_tasks,
            default_timeout_seconds,
            max_dependency_depth,
        },
    )
}

/// Generate arbitrary metadata (0 to 10 key-value pairs).
fn arb_metadata() -> impl Strategy<Value = HashMap<String, String>> {
    prop::collection::hash_map(
        "[a-zA-Z_][a-zA-Z0-9_]{0,20}",
        "[a-zA-Z0-9 _.-]{0,50}",
        0..10,
    )
}

/// Generate an arbitrary DateTime<Utc> within a reasonable range.
fn arb_datetime() -> impl Strategy<Value = DateTime<Utc>> {
    // Generate timestamps between 2020-01-01 and 2030-01-01
    (1577836800i64..1893456000i64).prop_map(|secs| Utc.timestamp_opt(secs, 0).unwrap())
}

/// Generate a complete arbitrary SessionState.
fn arb_session_state() -> impl Strategy<Value = SessionState> {
    (
        arb_session_id(),
        arb_task_queue(),
        arb_conversation_context(),
        arb_scheduler_config(),
        arb_datetime(),
        arb_metadata(),
    )
        .prop_map(
            |(
                session_id,
                task_queue,
                conversation_context,
                scheduler_config,
                persisted_at,
                metadata,
            )| {
                SessionState {
                    session_id,
                    task_queue,
                    conversation_context,
                    scheduler_config,
                    persisted_at,
                    metadata,
                }
            },
        )
}

// ============================================================================
// Test helpers
// ============================================================================

/// Create a test database with the session_state table.
async fn setup_test_db() -> (SqlitePool, TempDir) {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = tmp_dir.path().join("test_session_props.db");
    let pool = init_database(&db_path).await.expect("failed to init db");
    create_session_state_table(&pool)
        .await
        .expect("failed to create session_state table");
    (pool, tmp_dir)
}

// ============================================================================
// Property 3: Session State Persistence Round-Trip
//
// For any valid session state (including active task queue, conversation history,
// and scheduler configuration), persisting the state and then restoring it SHALL
// produce a state equivalent to the original.
//
// **Validates: Requirements 2.3**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// **Validates: Requirements 2.3**
    ///
    /// Property 3: For any valid session state, save followed by restore
    /// SHALL produce a state equivalent to the original.
    #[test]
    fn prop_session_state_round_trip(state in arb_session_state()) {
        // Run the async test in a tokio runtime
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (pool, _tmp) = setup_test_db().await;

            // Save the state
            save_session_state(&pool, &state)
                .await
                .expect("save_session_state should succeed");

            // Restore the state
            let restored = restore_session_state(&pool, state.session_id)
                .await
                .expect("restore_session_state should succeed")
                .expect("restored state should not be None");

            // Verify equivalence of all fields
            prop_assert_eq!(&restored.session_id, &state.session_id,
                "session_id mismatch");
            prop_assert_eq!(&restored.task_queue, &state.task_queue,
                "task_queue mismatch");
            prop_assert_eq!(&restored.conversation_context, &state.conversation_context,
                "conversation_context mismatch");
            prop_assert_eq!(
                restored.scheduler_config.max_concurrent_tasks,
                state.scheduler_config.max_concurrent_tasks,
                "scheduler_config.max_concurrent_tasks mismatch"
            );
            prop_assert_eq!(
                restored.scheduler_config.default_timeout_seconds,
                state.scheduler_config.default_timeout_seconds,
                "scheduler_config.default_timeout_seconds mismatch"
            );
            prop_assert_eq!(
                restored.scheduler_config.max_dependency_depth,
                state.scheduler_config.max_dependency_depth,
                "scheduler_config.max_dependency_depth mismatch"
            );
            prop_assert_eq!(&restored.persisted_at, &state.persisted_at,
                "persisted_at mismatch");
            prop_assert_eq!(&restored.metadata, &state.metadata,
                "metadata mismatch");

            Ok(())
        })?;
    }

    /// **Validates: Requirements 2.3**
    ///
    /// Property 3 (overwrite variant): For any two valid session states with the
    /// same session_id, saving the second state and restoring SHALL produce the
    /// second state (the first is overwritten).
    #[test]
    fn prop_session_state_overwrite_round_trip(
        state1 in arb_session_state(),
        state2_fields in (arb_task_queue(), arb_conversation_context(), arb_scheduler_config(), arb_datetime(), arb_metadata())
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (pool, _tmp) = setup_test_db().await;

            // Save the first state
            save_session_state(&pool, &state1)
                .await
                .expect("save first state should succeed");

            // Create a second state with the same session_id but different fields
            let state2 = SessionState {
                session_id: state1.session_id,
                task_queue: state2_fields.0,
                conversation_context: state2_fields.1,
                scheduler_config: state2_fields.2,
                persisted_at: state2_fields.3,
                metadata: state2_fields.4,
            };

            // Save the second state (should overwrite)
            save_session_state(&pool, &state2)
                .await
                .expect("save second state should succeed");

            // Restore should return the second state
            let restored = restore_session_state(&pool, state1.session_id)
                .await
                .expect("restore should succeed")
                .expect("restored state should not be None");

            prop_assert_eq!(&restored.session_id, &state2.session_id);
            prop_assert_eq!(&restored.task_queue, &state2.task_queue);
            prop_assert_eq!(&restored.conversation_context, &state2.conversation_context);
            prop_assert_eq!(
                restored.scheduler_config.max_concurrent_tasks,
                state2.scheduler_config.max_concurrent_tasks
            );
            prop_assert_eq!(
                restored.scheduler_config.default_timeout_seconds,
                state2.scheduler_config.default_timeout_seconds
            );
            prop_assert_eq!(
                restored.scheduler_config.max_dependency_depth,
                state2.scheduler_config.max_dependency_depth
            );
            prop_assert_eq!(&restored.persisted_at, &state2.persisted_at);
            prop_assert_eq!(&restored.metadata, &state2.metadata);

            Ok(())
        })?;
    }
}
