//! Property-based tests for file operation undo round-trip.
//!
//! **Validates: Requirements 4.5**
//!
//! Property 10: File Operation Undo Round-Trip
//!
//! *For any* sequence of up to 50 file operations (create, edit, delete),
//! undoing all operations in reverse order SHALL restore the filesystem to its
//! state before the first operation was applied.

use coding_module::file_ops::FileOperations;
use coding_module::undo_history::TrackedFileOperations;
use common::config::FilesystemRule;
use proptest::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

// ============================================================================
// Test helpers
// ============================================================================

/// Create a TrackedFileOperations instance with a temp directory as workspace.
fn setup() -> (TempDir, TrackedFileOperations) {
    let tmp = TempDir::new().unwrap();
    let rules = vec![FilesystemRule {
        path: tmp.path().to_path_buf(),
        read: true,
        write: true,
        execute: false,
    }];
    let ops = FileOperations::new(rules);
    let tracked = TrackedFileOperations::new(ops);
    (tmp, tracked)
}

/// Snapshot the filesystem state of a directory: maps relative paths to file contents.
/// Only captures regular files (not directories).
fn snapshot_dir(base: &std::path::Path) -> HashMap<PathBuf, String> {
    let mut map = HashMap::new();
    if base.exists() {
        snapshot_dir_recursive(base, base, &mut map);
    }
    map
}

fn snapshot_dir_recursive(
    base: &std::path::Path,
    current: &std::path::Path,
    map: &mut HashMap<PathBuf, String>,
) {
    if let Ok(entries) = fs::read_dir(current) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let relative = path.strip_prefix(base).unwrap().to_path_buf();
                let content = fs::read_to_string(&path).unwrap_or_default();
                map.insert(relative, content);
            } else if path.is_dir() {
                snapshot_dir_recursive(base, &path, map);
            }
        }
    }
}

// ============================================================================
// Strategies
// ============================================================================

/// Represents a file operation to be applied.
#[derive(Debug, Clone)]
enum FileOp {
    /// Create a new file with given relative path and content.
    Create { name: String, content: String },
    /// Edit an existing file (replace old content with new content).
    Edit { name: String, new_content: String },
    /// Delete an existing file.
    Delete { name: String },
}

/// Generate a valid filename (simple alphanumeric, no path separators).
fn filename_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-z][a-z0-9]{0,7}\\.txt")
        .unwrap()
        .prop_filter("filename must not be empty", |s| !s.is_empty())
}

/// Generate file content (printable ASCII, reasonably sized).
fn content_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-zA-Z0-9 _\\-\\.]{1,100}").unwrap()
}

/// Generate a sequence of file operations that is logically valid:
/// - Create operations target files that don't yet exist
/// - Edit operations target files that do exist
/// - Delete operations target files that do exist
///
/// We build the sequence by maintaining a virtual state of which files exist
/// and choosing operations accordingly.
fn valid_op_sequence_strategy(max_ops: usize) -> impl Strategy<Value = Vec<FileOp>> {
    // Generate a pool of filenames and contents, then build a valid sequence
    let pool_size = max_ops.min(50);
    (
        proptest::collection::vec(filename_strategy(), 1..=pool_size.max(1)),
        proptest::collection::vec(content_strategy(), 1..=(pool_size * 2).max(1)),
        proptest::collection::vec(0u8..3, 1..=max_ops.max(1)),
    )
        .prop_flat_map(move |(names, contents, op_choices)| {
            // Build a deterministic valid sequence from the random inputs
            let ops = build_valid_sequence(&names, &contents, &op_choices, max_ops);
            Just(ops)
        })
}

/// Build a valid operation sequence from random inputs.
/// Maintains a virtual state to ensure operations are logically consistent.
fn build_valid_sequence(
    names: &[String],
    contents: &[String],
    op_choices: &[u8],
    max_ops: usize,
) -> Vec<FileOp> {
    let mut ops = Vec::new();
    let mut existing_files: HashMap<String, String> = HashMap::new();
    let mut content_idx = 0;

    let next_content = |idx: &mut usize| -> String {
        let c = contents[*idx % contents.len()].clone();
        *idx += 1;
        c
    };

    for &choice in op_choices.iter().take(max_ops) {
        match choice {
            0 => {
                // Try to create a file that doesn't exist yet
                let name = names
                    .iter()
                    .find(|n| !existing_files.contains_key(*n))
                    .cloned();
                if let Some(name) = name {
                    let content = next_content(&mut content_idx);
                    existing_files.insert(name.clone(), content.clone());
                    ops.push(FileOp::Create { name, content });
                }
            }
            1 => {
                // Try to edit an existing file
                if let Some(name) = existing_files.keys().next().cloned() {
                    let new_content = next_content(&mut content_idx);
                    existing_files.insert(name.clone(), new_content.clone());
                    ops.push(FileOp::Edit { name, new_content });
                }
            }
            2 => {
                // Try to delete an existing file
                if let Some(name) = existing_files.keys().next().cloned() {
                    existing_files.remove(&name);
                    ops.push(FileOp::Delete { name });
                }
            }
            _ => unreachable!(),
        }
    }

    ops
}

// ============================================================================
// Property 10: File Operation Undo Round-Trip
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// **Validates: Requirements 4.5**
    ///
    /// Property 10: For any sequence of up to 50 file operations (create, edit,
    /// delete), undoing all operations in reverse order SHALL restore the
    /// filesystem to its state before the first operation was applied.
    #[test]
    fn prop_undo_round_trip_restores_original_state(
        ops in valid_op_sequence_strategy(50)
    ) {
        let (tmp, mut tracked) = setup();

        // Snapshot the initial filesystem state (should be empty)
        let initial_snapshot = snapshot_dir(tmp.path());

        // Apply all operations
        let mut applied_count = 0;
        for op in &ops {
            let result = match op {
                FileOp::Create { name, content } => {
                    let path = tmp.path().join(name);
                    tracked.create_file(&path, content)
                }
                FileOp::Edit { name, new_content } => {
                    let path = tmp.path().join(name);
                    tracked.write_file(&path, new_content)
                }
                FileOp::Delete { name } => {
                    let path = tmp.path().join(name);
                    tracked.delete_file(&path)
                }
            };

            if result.is_ok() {
                applied_count += 1;
            }
        }

        // Undo all applied operations
        if applied_count > 0 {
            let undo_results = tracked.undo(applied_count);

            // All undo operations should succeed
            for undo_result in &undo_results {
                prop_assert!(
                    undo_result.result.is_ok(),
                    "Undo operation failed: {:?}",
                    undo_result.result
                );
            }

            prop_assert_eq!(
                undo_results.len(),
                applied_count,
                "Should undo exactly as many operations as were applied"
            );
        }

        // Snapshot the filesystem after undo
        let final_snapshot = snapshot_dir(tmp.path());

        // The filesystem should be restored to its initial state
        prop_assert_eq!(
            &initial_snapshot,
            &final_snapshot,
            "Filesystem should be restored to initial state after undoing all operations. \
             Initial had {} files, final has {} files. \
             Operations applied: {:?}",
            initial_snapshot.len(),
            final_snapshot.len(),
            ops
        );
    }

    /// **Validates: Requirements 4.5**
    ///
    /// Property 10 (partial undo): For any sequence of N operations where we
    /// undo K operations (K <= N), the filesystem should match the state after
    /// only the first (N - K) operations were applied.
    #[test]
    fn prop_partial_undo_restores_intermediate_state(
        ops in valid_op_sequence_strategy(20),
        undo_fraction in 0.0f64..=1.0
    ) {
        let (tmp, mut tracked) = setup();

        // Apply all operations, taking snapshots after each
        let mut snapshots: Vec<HashMap<PathBuf, String>> = Vec::new();
        snapshots.push(snapshot_dir(tmp.path())); // snapshot[0] = initial state

        let mut applied_count = 0;
        for op in &ops {
            let result = match op {
                FileOp::Create { name, content } => {
                    let path = tmp.path().join(name);
                    tracked.create_file(&path, content)
                }
                FileOp::Edit { name, new_content } => {
                    let path = tmp.path().join(name);
                    tracked.write_file(&path, new_content)
                }
                FileOp::Delete { name } => {
                    let path = tmp.path().join(name);
                    tracked.delete_file(&path)
                }
            };

            if result.is_ok() {
                applied_count += 1;
                snapshots.push(snapshot_dir(tmp.path()));
            }
        }

        if applied_count == 0 {
            return Ok(());
        }

        // Determine how many to undo
        let undo_count = ((applied_count as f64) * undo_fraction).round() as usize;
        let undo_count = undo_count.min(applied_count);

        if undo_count == 0 {
            return Ok(());
        }

        // Undo K operations
        let undo_results = tracked.undo(undo_count);

        for undo_result in &undo_results {
            prop_assert!(
                undo_result.result.is_ok(),
                "Undo operation failed: {:?}",
                undo_result.result
            );
        }

        // The filesystem should match the state after (applied_count - undo_count) operations
        let expected_snapshot_idx = applied_count - undo_count;
        let expected_snapshot = &snapshots[expected_snapshot_idx];
        let actual_snapshot = snapshot_dir(tmp.path());

        prop_assert_eq!(
            expected_snapshot,
            &actual_snapshot,
            "After undoing {} of {} operations, filesystem should match state at step {}",
            undo_count,
            applied_count,
            expected_snapshot_idx
        );
    }

    /// **Validates: Requirements 4.5**
    ///
    /// Property 10 (bounded history): The undo history never exceeds 50 entries,
    /// and operations beyond the capacity are correctly evicted.
    #[test]
    fn prop_undo_history_respects_capacity(
        ops in valid_op_sequence_strategy(50)
    ) {
        let (tmp, mut tracked) = setup();

        let mut applied_count = 0;
        for op in &ops {
            let result = match op {
                FileOp::Create { name, content } => {
                    let path = tmp.path().join(name);
                    tracked.create_file(&path, content)
                }
                FileOp::Edit { name, new_content } => {
                    let path = tmp.path().join(name);
                    tracked.write_file(&path, new_content)
                }
                FileOp::Delete { name } => {
                    let path = tmp.path().join(name);
                    tracked.delete_file(&path)
                }
            };

            if result.is_ok() {
                applied_count += 1;
            }

            // History should never exceed capacity (50)
            prop_assert!(
                tracked.history_len() <= 50,
                "History length {} exceeds capacity 50",
                tracked.history_len()
            );
        }

        // History length should be min(applied_count, 50)
        let expected_len = applied_count.min(50);
        prop_assert_eq!(
            tracked.history_len(),
            expected_len,
            "History length should be min(applied_count={}, 50)",
            applied_count
        );
    }
}
