//! Property-based tests for file edit string replacement correctness.
//!
//! **Validates: Requirements 4.2, 4.6**
//!
//! Property 9: File Edit String Replacement Correctness
//!
//! *For any* file content and a (old_str, new_str) pair where old_str exists
//! exactly once in the content, applying the edit SHALL produce content where
//! old_str is replaced by new_str and all other content is unchanged. If old_str
//! does not exist in the content, the edit SHALL be rejected.

use coding_module::file_ops::FileOperations;
use common::config::FilesystemRule;
use proptest::prelude::*;
use std::fs;
use tempfile::TempDir;

// ============================================================================
// Test helpers
// ============================================================================

/// Create a FileOperations instance with a temp directory as the workspace.
fn setup() -> (TempDir, FileOperations) {
    let tmp = TempDir::new().unwrap();
    let rules = vec![FilesystemRule {
        path: tmp.path().to_path_buf(),
        read: true,
        write: true,
        execute: false,
    }];
    let ops = FileOperations::new(rules);
    (tmp, ops)
}

// ============================================================================
// Strategies
// ============================================================================

/// Generate arbitrary non-empty file content (printable ASCII + whitespace).
/// We keep content reasonably sized to avoid slow tests.
fn file_content_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[\\x20-\\x7E\\n\\t]{1,500}")
        .unwrap()
        .prop_filter("content must not be empty", |s| !s.is_empty())
}

/// Generate a non-empty old_str that we will ensure exists in the content.
fn old_str_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[\\x20-\\x7E]{1,50}")
        .unwrap()
        .prop_filter("old_str must not be empty", |s| !s.is_empty())
}

/// Generate an arbitrary new_str replacement (can be empty — deletion is valid).
fn new_str_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[\\x20-\\x7E\\n\\t]{0,100}").unwrap()
}

/// Strategy that produces (content, old_str, new_str) where old_str appears
/// exactly once in content. We construct this by generating a prefix, suffix,
/// old_str, and new_str, then composing content = prefix + old_str + suffix,
/// filtering out cases where old_str also appears in prefix or suffix.
fn single_occurrence_edit_strategy() -> impl Strategy<Value = (String, String, String)> {
    (
        proptest::string::string_regex("[a-zA-Z0-9 ]{0,200}").unwrap(),
        proptest::string::string_regex("[a-zA-Z0-9_]{1,30}").unwrap(),
        proptest::string::string_regex("[a-zA-Z0-9 ]{0,200}").unwrap(),
        new_str_strategy(),
    )
        .prop_filter_map(
            "old_str must appear exactly once in content",
            |(prefix, old_str, suffix, new_str)| {
                // Ensure old_str doesn't appear in prefix or suffix
                if prefix.contains(&old_str) || suffix.contains(&old_str) {
                    return None;
                }
                let content = format!("{}{}{}", prefix, old_str, suffix);
                // Double-check: old_str appears exactly once
                if content.matches(&old_str).count() != 1 {
                    return None;
                }
                Some((content, old_str, new_str))
            },
        )
}

/// Strategy that produces (content, old_str, new_str) where old_str appears
/// multiple times in content (at least 2).
fn multiple_occurrence_edit_strategy() -> impl Strategy<Value = (String, String, String)> {
    (
        proptest::string::string_regex("[a-zA-Z0-9_]{1,20}").unwrap(),
        proptest::string::string_regex("[a-zA-Z0-9 ]{0,50}").unwrap(),
        new_str_strategy(),
    )
        .prop_filter_map(
            "old_str must appear multiple times",
            |(old_str, separator, new_str)| {
                if old_str.is_empty() {
                    return None;
                }
                // Ensure separator doesn't contain old_str
                if separator.contains(&old_str) {
                    return None;
                }
                // Build content with old_str appearing exactly twice
                let content = format!("{}{}{}", old_str, separator, old_str);
                if content.matches(&old_str).count() < 2 {
                    return None;
                }
                Some((content, old_str, new_str))
            },
        )
}

/// Strategy that produces (content, old_str) where old_str does NOT exist
/// in content.
fn missing_old_str_strategy() -> impl Strategy<Value = (String, String)> {
    (file_content_strategy(), old_str_strategy()).prop_filter(
        "old_str must not exist in content",
        |(content, old_str)| !content.contains(old_str.as_str()),
    )
}

// ============================================================================
// Property 9: File Edit String Replacement Correctness
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 4.2, 4.6**
    ///
    /// Property 9: When old_str exists exactly once in the file content,
    /// applying the edit SHALL produce content where old_str is replaced by
    /// new_str and all other content is unchanged.
    #[test]
    fn prop_edit_replaces_single_occurrence_correctly(
        (content, old_str, new_str) in single_occurrence_edit_strategy()
    ) {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("test_file.txt");
        fs::write(&file_path, &content).unwrap();

        let result = ops.edit_file(&file_path, &old_str, &new_str);
        prop_assert!(result.is_ok(), "edit_file should succeed when old_str exists: {:?}", result.err());

        let new_content = result.unwrap();

        // The result should be the content with old_str replaced by new_str
        let expected = content.replacen(&old_str, &new_str, 1);
        prop_assert_eq!(&new_content, &expected,
            "Content after edit should match expected replacement");

        // Verify the file on disk matches
        let on_disk = fs::read_to_string(&file_path).unwrap();
        prop_assert_eq!(&on_disk, &expected,
            "File on disk should match expected replacement");

        // Verify old_str is no longer present (since it appeared exactly once)
        prop_assert!(!new_content.contains(&old_str) || old_str == new_str || new_str.contains(&old_str),
            "old_str should not appear in result unless new_str contains it");
    }

    /// **Validates: Requirements 4.2, 4.6**
    ///
    /// Property 9: When old_str does NOT exist in the file content, the edit
    /// SHALL be rejected with an error.
    #[test]
    fn prop_edit_rejects_when_old_str_not_found(
        (content, old_str) in missing_old_str_strategy()
    ) {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("test_file.txt");
        fs::write(&file_path, &content).unwrap();

        let result = ops.edit_file(&file_path, &old_str, "any_replacement");
        prop_assert!(result.is_err(),
            "edit_file should fail when old_str is not in content");

        let err = result.unwrap_err();
        prop_assert!(err.contains("not found"),
            "Error message should indicate match string was not found, got: {}", err);

        // Verify the file content is unchanged
        let on_disk = fs::read_to_string(&file_path).unwrap();
        prop_assert_eq!(&on_disk, &content,
            "File content should be unchanged when edit is rejected");
    }

    /// **Validates: Requirements 4.2, 4.6**
    ///
    /// Property 9: When old_str appears multiple times in the content, only
    /// the first occurrence SHALL be replaced.
    #[test]
    fn prop_edit_replaces_only_first_occurrence(
        (content, old_str, new_str) in multiple_occurrence_edit_strategy()
    ) {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("test_file.txt");
        fs::write(&file_path, &content).unwrap();

        let occurrences_before = content.matches(&old_str).count();
        prop_assert!(occurrences_before >= 2,
            "Precondition: old_str should appear at least twice");

        let result = ops.edit_file(&file_path, &old_str, &new_str);
        prop_assert!(result.is_ok(), "edit_file should succeed when old_str exists: {:?}", result.err());

        let new_content = result.unwrap();

        // The result should match replacen with count=1
        let expected = content.replacen(&old_str, &new_str, 1);
        prop_assert_eq!(&new_content, &expected,
            "Content should match single-replacement result");

        // If new_str doesn't contain old_str, remaining occurrences should be
        // exactly (occurrences_before - 1)
        if !new_str.contains(&old_str) {
            let occurrences_after = new_content.matches(&old_str).count();
            prop_assert_eq!(occurrences_after, occurrences_before - 1,
                "After replacing first occurrence, remaining count should be one less");
        }

        // Verify the file on disk matches
        let on_disk = fs::read_to_string(&file_path).unwrap();
        prop_assert_eq!(&on_disk, &expected,
            "File on disk should match expected single-replacement result");
    }

    /// **Validates: Requirements 4.2, 4.6**
    ///
    /// Property 9: The edit operation preserves all content outside the
    /// replaced region. Specifically, the prefix before old_str and the suffix
    /// after old_str remain unchanged.
    #[test]
    fn prop_edit_preserves_surrounding_content(
        (content, old_str, new_str) in single_occurrence_edit_strategy()
    ) {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("test_file.txt");
        fs::write(&file_path, &content).unwrap();

        // Find the position of old_str in content
        let pos = content.find(&old_str).unwrap();
        let prefix = &content[..pos];
        let suffix = &content[pos + old_str.len()..];

        let result = ops.edit_file(&file_path, &old_str, &new_str).unwrap();

        // Verify prefix is preserved
        prop_assert!(result.starts_with(prefix),
            "Prefix should be preserved. Expected start: {:?}, got: {:?}",
            prefix, &result[..prefix.len().min(result.len())]);

        // Verify suffix is preserved
        prop_assert!(result.ends_with(suffix),
            "Suffix should be preserved. Expected end: {:?}, got: {:?}",
            suffix, &result[result.len().saturating_sub(suffix.len())..]);

        // Verify the middle is exactly new_str
        let middle = &result[prefix.len()..result.len() - suffix.len()];
        prop_assert_eq!(middle, &new_str,
            "Middle section should be exactly new_str");
    }
}
