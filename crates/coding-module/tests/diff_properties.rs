//! Property-based tests for diff generation and change tracking.
//!
//! **Validates: Requirements 16.2, 16.5, 16.7**
//!
//! Property 31: Unified Diff Generation Correctness
//! Property 32: Cumulative Diff Equivalence
//! Property 33: Session Change Summary Accuracy

use coding_module::{
    generate_unified_diff, ChangeTracker, DiffLine, UnifiedDiff,
};
use proptest::prelude::*;
use std::path::Path;

// ============================================================================
// Strategies
// ============================================================================

/// Generate arbitrary text content as lines (non-binary, printable ASCII + newlines).
/// Each line is a printable ASCII string, joined by newlines.
fn text_content_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(
        proptest::string::string_regex("[a-zA-Z0-9 _.,;:!?()\\-]{0,80}").unwrap(),
        0..30,
    )
    .prop_map(|lines| {
        if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n") + "\n"
        }
    })
}

/// Generate non-empty text content (at least one line).
fn non_empty_text_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(
        proptest::string::string_regex("[a-zA-Z0-9 _.,;:!?()\\-]{1,60}").unwrap(),
        1..20,
    )
    .prop_map(|lines| lines.join("\n") + "\n")
}

/// Generate a pair of (before, after) text contents that are different.
fn diff_pair_strategy() -> impl Strategy<Value = (String, String)> {
    (text_content_strategy(), text_content_strategy()).prop_filter(
        "before and after must differ",
        |(before, after)| before != after,
    )
}

/// Generate a file path (simple name for testing).
fn file_path_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("/tmp/[a-z]{1,10}\\.txt")
        .unwrap()
}

/// Generate a sequence of modifications to a single file (2-5 edits).
fn modification_sequence_strategy() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(non_empty_text_strategy(), 3..6)
}

/// Generate multiple file modifications for session summary testing.
/// Returns Vec<(path, original_content, final_content)>.
fn multi_file_changes_strategy() -> impl Strategy<Value = Vec<(String, String, String)>> {
    prop::collection::vec(
        (
            file_path_strategy(),
            non_empty_text_strategy(),
            non_empty_text_strategy(),
        ),
        1..6,
    )
    .prop_filter("paths must be unique", |changes| {
        let mut paths: Vec<&str> = changes.iter().map(|(p, _, _)| p.as_str()).collect();
        paths.sort();
        paths.dedup();
        paths.len() == changes.len()
    })
    .prop_filter("at least one file must have different before/after", |changes| {
        changes.iter().any(|(_, before, after)| before != after)
    })
}

// ============================================================================
// Helper: Apply a unified diff to the original content to verify correctness
// ============================================================================

/// Apply a unified diff to the original content and return the result.
/// This is a simple patch application that verifies the diff is correct.
fn apply_diff(original: &str, diff: &UnifiedDiff) -> Result<String, String> {
    if diff.hunks.is_empty() {
        // No changes — return original
        return Ok(original.to_string());
    }

    let original_lines: Vec<&str> = if original.is_empty() {
        Vec::new()
    } else {
        original.lines().collect()
    };

    let mut result_lines: Vec<String> = Vec::new();
    let mut current_old_line = 0; // 0-indexed position in original_lines

    for hunk in &diff.hunks {
        // The hunk starts at old_start (1-based), so we need to copy lines
        // from current_old_line up to (old_start - 1) as unchanged.
        let hunk_start = if hunk.old_start == 0 {
            0
        } else {
            hunk.old_start - 1
        };

        // Copy unchanged lines before this hunk
        while current_old_line < hunk_start {
            if current_old_line < original_lines.len() {
                result_lines.push(original_lines[current_old_line].to_string());
            }
            current_old_line += 1;
        }

        // Process hunk lines
        for line in &hunk.lines {
            match line {
                DiffLine::Context(content) => {
                    result_lines.push(content.clone());
                    current_old_line += 1;
                }
                DiffLine::Addition(content) => {
                    result_lines.push(content.clone());
                    // Additions don't consume original lines
                }
                DiffLine::Removal(_) => {
                    // Removals consume an original line but don't add to result
                    current_old_line += 1;
                }
            }
        }
    }

    // Copy any remaining lines after the last hunk
    while current_old_line < original_lines.len() {
        result_lines.push(original_lines[current_old_line].to_string());
        current_old_line += 1;
    }

    // Reconstruct with newlines
    if result_lines.is_empty() {
        Ok(String::new())
    } else {
        Ok(result_lines.join("\n") + "\n")
    }
}

// ============================================================================
// Property 31: Unified Diff Generation Correctness
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 16.2**
    ///
    /// Property 31: For any pair of (before, after) file contents, the generated
    /// unified diff SHALL be a valid unified diff format with at least 3 context
    /// lines, and applying the diff to the before content SHALL produce the after
    /// content.
    #[test]
    fn prop_unified_diff_correctness(
        (before, after) in diff_pair_strategy()
    ) {
        let path = Path::new("test.txt");
        let diff = generate_unified_diff(path, path, &before, &after, 3);

        // The diff should not be marked as binary
        prop_assert!(!diff.is_binary, "Text content should not produce binary diff");

        // The diff should have at least one hunk (since before != after)
        prop_assert!(!diff.hunks.is_empty(),
            "Diff between different contents should have at least one hunk");

        // Verify lines_added and lines_removed are consistent with hunk content
        let mut counted_added = 0;
        let mut counted_removed = 0;
        for hunk in &diff.hunks {
            for line in &hunk.lines {
                match line {
                    DiffLine::Addition(_) => counted_added += 1,
                    DiffLine::Removal(_) => counted_removed += 1,
                    DiffLine::Context(_) => {}
                }
            }
        }
        prop_assert_eq!(diff.lines_added, counted_added,
            "lines_added should match counted additions in hunks");
        prop_assert_eq!(diff.lines_removed, counted_removed,
            "lines_removed should match counted removals in hunks");

        // Apply the diff to the before content and verify it produces the after content
        let applied = apply_diff(&before, &diff)
            .map_err(|e| TestCaseError::Fail(format!("Failed to apply diff: {}", e).into()))?;
        prop_assert_eq!(&applied, &after,
            "Applying the diff to before content should produce after content.\n\
             Before: {:?}\nAfter: {:?}\nApplied: {:?}\nDiff: {:?}",
            before, after, applied, diff);
    }

    /// **Validates: Requirements 16.2**
    ///
    /// Property 31: Context lines — each hunk in the diff should have context
    /// lines (at least 3 where available) around changes.
    #[test]
    fn prop_unified_diff_has_context_lines(
        (before, after) in diff_pair_strategy()
    ) {
        let path = Path::new("test.txt");
        let diff = generate_unified_diff(path, path, &before, &after, 3);

        let before_line_count = if before.is_empty() { 0 } else { before.lines().count() };

        for hunk in &diff.hunks {
            let context_count = hunk.lines.iter()
                .filter(|l| matches!(l, DiffLine::Context(_)))
                .count();

            // If the file has enough lines, we should have context lines
            // (at least some context unless the entire file is changed)
            if before_line_count > 6 {
                // With a file of more than 6 lines and at least 3 context lines
                // configured, we expect at least some context in each hunk
                // (unless the entire file is one big change)
                let total_changes = hunk.lines.iter()
                    .filter(|l| !matches!(l, DiffLine::Context(_)))
                    .count();
                if total_changes < before_line_count {
                    prop_assert!(context_count > 0,
                        "Hunks should have context lines when file has enough lines. \
                         Hunk has {} changes and {} context lines in a {}-line file",
                        total_changes, context_count, before_line_count);
                }
            }
        }
    }

    /// **Validates: Requirements 16.2**
    ///
    /// Property 31: For identical contents, the diff should have no hunks and
    /// zero additions/removals.
    #[test]
    fn prop_unified_diff_identical_contents_empty(
        content in text_content_strategy()
    ) {
        let path = Path::new("test.txt");
        let diff = generate_unified_diff(path, path, &content, &content, 3);

        prop_assert!(diff.hunks.is_empty(),
            "Diff of identical content should have no hunks");
        prop_assert_eq!(diff.lines_added, 0,
            "Diff of identical content should have 0 lines added");
        prop_assert_eq!(diff.lines_removed, 0,
            "Diff of identical content should have 0 lines removed");
    }

    /// **Validates: Requirements 16.2**
    ///
    /// Property 31: For a new file (empty before), all lines should be additions.
    #[test]
    fn prop_unified_diff_new_file_all_additions(
        content in non_empty_text_strategy()
    ) {
        let path = Path::new("new.txt");
        let diff = generate_unified_diff(path, path, "", &content, 3);

        let line_count = content.lines().count();
        prop_assert_eq!(diff.lines_added, line_count,
            "New file diff should have all lines as additions");
        prop_assert_eq!(diff.lines_removed, 0,
            "New file diff should have 0 removals");
    }

    /// **Validates: Requirements 16.2**
    ///
    /// Property 31: For a deleted file (empty after), all lines should be removals.
    #[test]
    fn prop_unified_diff_deleted_file_all_removals(
        content in non_empty_text_strategy()
    ) {
        let path = Path::new("deleted.txt");
        let diff = generate_unified_diff(path, path, &content, "", 3);

        let line_count = content.lines().count();
        prop_assert_eq!(diff.lines_removed, line_count,
            "Deleted file diff should have all lines as removals");
        prop_assert_eq!(diff.lines_added, 0,
            "Deleted file diff should have 0 additions");
    }
}

// ============================================================================
// Property 32: Cumulative Diff Equivalence
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 16.7**
    ///
    /// Property 32: For any file modified N times during a session, the cumulative
    /// diff SHALL be equivalent to a single diff between the original state and
    /// the final state.
    #[test]
    fn prop_cumulative_diff_equivalence(
        states in modification_sequence_strategy()
    ) {
        let path = Path::new("/tmp/cumulative_test.txt");
        let mut tracker = ChangeTracker::new();

        // Record each modification in sequence
        for i in 0..states.len() - 1 {
            let before = &states[i];
            let after = &states[i + 1];
            tracker.record_change(path, Some(before), Some(after));
        }

        // Get the cumulative diff from the tracker
        let cumulative_diff = tracker.get_cumulative_diff(path).unwrap();

        // Generate a direct diff between original and final state
        let original = &states[0];
        let final_state = states.last().unwrap();
        let direct_diff = generate_unified_diff(path, path, original, final_state, 3);

        // The cumulative diff should have the same additions and removals
        prop_assert_eq!(
            cumulative_diff.lines_added, direct_diff.lines_added,
            "Cumulative diff lines_added ({}) should equal direct diff lines_added ({})",
            cumulative_diff.lines_added, direct_diff.lines_added
        );
        prop_assert_eq!(
            cumulative_diff.lines_removed, direct_diff.lines_removed,
            "Cumulative diff lines_removed ({}) should equal direct diff lines_removed ({})",
            cumulative_diff.lines_removed, direct_diff.lines_removed
        );

        // Both diffs should produce the same result when applied to the original
        let applied_cumulative = apply_diff(original, &cumulative_diff)
            .map_err(|e| TestCaseError::Fail(format!("Failed to apply cumulative diff: {}", e).into()))?;
        let applied_direct = apply_diff(original, &direct_diff)
            .map_err(|e| TestCaseError::Fail(format!("Failed to apply direct diff: {}", e).into()))?;

        prop_assert_eq!(&applied_cumulative, &applied_direct,
            "Applying cumulative diff and direct diff should produce the same result");
        prop_assert_eq!(&applied_cumulative, final_state,
            "Applied cumulative diff should equal the final state");
    }

    /// **Validates: Requirements 16.7**
    ///
    /// Property 32: When a file is modified back to its original state, the
    /// cumulative diff should be empty (no changes).
    #[test]
    fn prop_cumulative_diff_roundtrip_empty(
        original in non_empty_text_strategy(),
        intermediate in non_empty_text_strategy()
    ) {
        prop_assume!(original != intermediate);

        let path = Path::new("/tmp/roundtrip_test.txt");
        let mut tracker = ChangeTracker::new();

        // Modify: original -> intermediate -> original
        tracker.record_change(path, Some(&original), Some(&intermediate));
        tracker.record_change(path, Some(&intermediate), Some(&original));

        let diff = tracker.get_cumulative_diff(path).unwrap();

        // Since we went back to original, the cumulative diff should show no changes
        prop_assert_eq!(diff.lines_added, 0,
            "Roundtrip should have 0 additions, got {}", diff.lines_added);
        prop_assert_eq!(diff.lines_removed, 0,
            "Roundtrip should have 0 removals, got {}", diff.lines_removed);
        prop_assert!(diff.hunks.is_empty(),
            "Roundtrip should have no hunks");
    }
}

// ============================================================================
// Property 33: Session Change Summary Accuracy
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 16.5**
    ///
    /// Property 33: For any set of file modifications in a session, the session
    /// summary SHALL accurately report the total lines added and removed across
    /// all files, and list every modified file exactly once.
    #[test]
    fn prop_session_summary_accuracy(
        changes in multi_file_changes_strategy()
    ) {
        let mut tracker = ChangeTracker::new();

        // Record all changes
        for (path_str, before, after) in &changes {
            let path = Path::new(path_str);
            tracker.record_change(path, Some(before), Some(after));
        }

        let summary = tracker.session_summary();

        // Every modified file should appear exactly once in the summary
        prop_assert_eq!(summary.modified_files.len(), changes.len(),
            "Summary should list exactly {} files, got {}",
            changes.len(), summary.modified_files.len());

        for (path_str, _, _) in &changes {
            let path = std::path::PathBuf::from(path_str);
            prop_assert!(summary.modified_files.contains(&path),
                "Summary should contain file {:?}", path_str);
        }

        // Verify total lines added/removed matches individual diffs
        let mut expected_added = 0;
        let mut expected_removed = 0;
        for (path_str, before, after) in &changes {
            let path = Path::new(path_str);
            let diff = generate_unified_diff(path, path, before, after, 3);
            expected_added += diff.lines_added;
            expected_removed += diff.lines_removed;
        }

        prop_assert_eq!(summary.total_lines_added, expected_added,
            "Total lines added should be {}, got {}",
            expected_added, summary.total_lines_added);
        prop_assert_eq!(summary.total_lines_removed, expected_removed,
            "Total lines removed should be {}, got {}",
            expected_removed, summary.total_lines_removed);
    }

    /// **Validates: Requirements 16.5**
    ///
    /// Property 33: For multiple modifications to the same file, the session
    /// summary should report the cumulative lines added/removed (original vs final).
    #[test]
    fn prop_session_summary_cumulative_for_same_file(
        states in modification_sequence_strategy()
    ) {
        let path = Path::new("/tmp/multi_edit.txt");
        let mut tracker = ChangeTracker::new();

        // Record each modification in sequence
        for i in 0..states.len() - 1 {
            tracker.record_change(path, Some(&states[i]), Some(&states[i + 1]));
        }

        let summary = tracker.session_summary();

        // Should list the file exactly once
        prop_assert_eq!(summary.modified_files.len(), 1,
            "Summary should list exactly 1 file for multiple edits to same file");
        prop_assert!(summary.modified_files.contains(&path.to_path_buf()),
            "Summary should contain the modified file path");

        // The lines added/removed should match the diff between original and final
        let original = &states[0];
        let final_state = states.last().unwrap();
        let direct_diff = generate_unified_diff(path, path, original, final_state, 3);

        prop_assert_eq!(summary.total_lines_added, direct_diff.lines_added,
            "Summary lines_added ({}) should match direct diff ({})",
            summary.total_lines_added, direct_diff.lines_added);
        prop_assert_eq!(summary.total_lines_removed, direct_diff.lines_removed,
            "Summary lines_removed ({}) should match direct diff ({})",
            summary.total_lines_removed, direct_diff.lines_removed);
    }

    /// **Validates: Requirements 16.5**
    ///
    /// Property 33: An empty session (no changes) should produce an empty summary.
    #[test]
    fn prop_session_summary_empty_when_no_changes(_ in 0..1u8) {
        let tracker = ChangeTracker::new();
        let summary = tracker.session_summary();

        prop_assert!(summary.modified_files.is_empty(),
            "Empty session should have no modified files");
        prop_assert_eq!(summary.total_lines_added, 0,
            "Empty session should have 0 lines added");
        prop_assert_eq!(summary.total_lines_removed, 0,
            "Empty session should have 0 lines removed");
    }
}
