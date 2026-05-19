//! Property-based tests for knowledge storage validation and capacity enforcement.
//!
//! **Validates: Requirements 14.5, 14.8**
//!
//! Property 29: Knowledge Entry Size Validation
//! Property 30: Memory Store Capacity Enforcement

use std::collections::HashMap;

use memory_store::{init_database, KnowledgeStore, MAX_CONTENT_SIZE};
use proptest::prelude::*;
use tempfile::TempDir;

// ============================================================================
// Test helpers
// ============================================================================

/// Create a KnowledgeStore with default capacity backed by a temp database.
async fn setup_store() -> (KnowledgeStore, TempDir) {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = tmp_dir.path().join("test_knowledge_props.db");
    let pool = init_database(&db_path).await.expect("failed to init db");
    let store = KnowledgeStore::with_defaults(pool);
    (store, tmp_dir)
}

/// Create a KnowledgeStore with a specific capacity limit.
async fn setup_store_with_capacity(max_entries: u64) -> (KnowledgeStore, TempDir) {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = tmp_dir.path().join("test_knowledge_props.db");
    let pool = init_database(&db_path).await.expect("failed to init db");
    let store = KnowledgeStore::new(pool, max_entries);
    (store, tmp_dir)
}

// ============================================================================
// Strategies
// ============================================================================

/// Generate content strings that exceed the MAX_CONTENT_SIZE limit.
/// Produces strings of length MAX_CONTENT_SIZE+1 to MAX_CONTENT_SIZE+5000.
fn oversized_content_strategy() -> impl Strategy<Value = String> {
    ((MAX_CONTENT_SIZE + 1)..=(MAX_CONTENT_SIZE + 5000)).prop_flat_map(|len| {
        proptest::collection::vec(b'a'..=b'z', len).prop_map(|bytes| {
            bytes.into_iter().map(|b| b as char).collect::<String>()
        })
    })
}

/// Generate content strings that are within the MAX_CONTENT_SIZE limit.
/// Produces strings of length 0 to MAX_CONTENT_SIZE.
fn valid_content_strategy() -> impl Strategy<Value = String> {
    (0..=MAX_CONTENT_SIZE).prop_flat_map(|len| {
        proptest::collection::vec(b'a'..=b'z', len).prop_map(|bytes| {
            bytes.into_iter().map(|b| b as char).collect::<String>()
        })
    })
}

/// Generate content strings near the boundary (within 100 chars of MAX_CONTENT_SIZE).
fn boundary_content_strategy() -> impl Strategy<Value = usize> {
    (MAX_CONTENT_SIZE.saturating_sub(100))..=(MAX_CONTENT_SIZE + 100)
}

/// Generate a small capacity value for capacity enforcement tests.
fn small_capacity_strategy() -> impl Strategy<Value = u64> {
    1..=10u64
}

// ============================================================================
// Property 29: Knowledge Entry Size Validation
//
// For any content string exceeding 10,000 characters, store_knowledge() SHALL
// reject it with ContentTooLarge. For any content string of 10,000 characters
// or fewer, it SHALL accept it.
//
// **Validates: Requirements 14.5**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// **Validates: Requirements 14.5**
    ///
    /// Property 29: For any content string exceeding 10,000 characters,
    /// store_knowledge() SHALL reject it with ContentTooLarge.
    #[test]
    fn prop_oversized_content_rejected(
        content in oversized_content_strategy()
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _tmp) = setup_store().await;

            let result = store
                .store_knowledge("Test Entry", &content, &[], &HashMap::new())
                .await;

            prop_assert!(
                matches!(
                    &result,
                    Err(common::errors::StoreError::ContentTooLarge { max, actual })
                    if *max == MAX_CONTENT_SIZE && *actual == content.len()
                ),
                "Content of length {} should be rejected with ContentTooLarge, got: {:?}",
                content.len(),
                result
            );
            Ok(())
        })?;
    }

    /// **Validates: Requirements 14.5**
    ///
    /// Property 29: For any content string of 10,000 characters or fewer,
    /// store_knowledge() SHALL accept it.
    #[test]
    fn prop_valid_content_accepted(
        content in valid_content_strategy()
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _tmp) = setup_store().await;

            let result = store
                .store_knowledge("Test Entry", &content, &[], &HashMap::new())
                .await;

            prop_assert!(
                result.is_ok(),
                "Content of length {} (≤ {}) should be accepted, got: {:?}",
                content.len(),
                MAX_CONTENT_SIZE,
                result
            );
            Ok(())
        })?;
    }

    /// **Validates: Requirements 14.5**
    ///
    /// Property 29: Boundary test — content at exactly MAX_CONTENT_SIZE is accepted,
    /// content at MAX_CONTENT_SIZE+1 is rejected.
    #[test]
    fn prop_boundary_content_validation(
        len in boundary_content_strategy()
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _tmp) = setup_store().await;

            let content: String = std::iter::repeat('x').take(len).collect();
            let result = store
                .store_knowledge("Boundary Test", &content, &[], &HashMap::new())
                .await;

            if len <= MAX_CONTENT_SIZE {
                prop_assert!(
                    result.is_ok(),
                    "Content of length {} (≤ {}) should be accepted, got: {:?}",
                    len,
                    MAX_CONTENT_SIZE,
                    result
                );
            } else {
                prop_assert!(
                    matches!(
                        &result,
                        Err(common::errors::StoreError::ContentTooLarge { max, actual })
                        if *max == MAX_CONTENT_SIZE && *actual == len
                    ),
                    "Content of length {} (> {}) should be rejected with ContentTooLarge, got: {:?}",
                    len,
                    MAX_CONTENT_SIZE,
                    result
                );
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// Property 30: Memory Store Capacity Enforcement
//
// For any store at its configured capacity limit, attempting to store a new
// entry SHALL be rejected with CapacityFull without deleting existing entries.
//
// **Validates: Requirements 14.8**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20))]

    /// **Validates: Requirements 14.8**
    ///
    /// Property 30: For any store at its configured capacity limit, attempting
    /// to store a new entry SHALL be rejected with CapacityFull without deleting
    /// existing entries.
    #[test]
    fn prop_capacity_full_rejects_new_entries(
        capacity in small_capacity_strategy()
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _tmp) = setup_store_with_capacity(capacity).await;

            // Fill the store to capacity
            for i in 0..capacity {
                let result = store
                    .store_knowledge(
                        &format!("Entry {}", i),
                        &format!("Content for entry {}", i),
                        &[],
                        &HashMap::new(),
                    )
                    .await;
                prop_assert!(
                    result.is_ok(),
                    "Filling store: entry {} of {} should succeed, got: {:?}",
                    i,
                    capacity,
                    result
                );
            }

            // Verify the store is at capacity
            let count = store.entry_count().await.unwrap();
            prop_assert_eq!(count, capacity, "Store should be at capacity");

            // Attempt to store one more entry — should be rejected
            let overflow_result = store
                .store_knowledge("Overflow Entry", "overflow content", &[], &HashMap::new())
                .await;

            prop_assert!(
                matches!(&overflow_result, Err(common::errors::StoreError::CapacityFull)),
                "Store at capacity {} should reject new entries with CapacityFull, got: {:?}",
                capacity,
                overflow_result
            );

            // Verify existing entries are still intact (not deleted)
            let count_after = store.entry_count().await.unwrap();
            prop_assert_eq!(
                count_after, capacity,
                "Existing entries should remain intact after CapacityFull rejection"
            );

            // Verify we can still search and find existing entries
            let results = store.search_knowledge("Entry", capacity as u32).await.unwrap();
            prop_assert_eq!(
                results.len() as u64,
                capacity,
                "All {} existing entries should still be searchable after rejection",
                capacity
            );

            Ok(())
        })?;
    }
}
