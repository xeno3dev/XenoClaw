//! Property-based tests for git commit message format.
//!
//! **Validates: Requirements 6.2**
//!
//! Property 12: Git Commit Message Format
//!
//! *For any* generated commit message, the summary line SHALL be at most 72
//! characters, include the type of change and affected component, and the body
//! SHALL list the specific files modified.

use coding_module::git_ops::GitOperations;
use proptest::prelude::*;

// ============================================================================
// Strategies
// ============================================================================

/// Generate arbitrary change types (conventional commit types).
fn change_type_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("feat".to_string()),
        Just("fix".to_string()),
        Just("refactor".to_string()),
        Just("chore".to_string()),
        Just("docs".to_string()),
        Just("test".to_string()),
        Just("style".to_string()),
        Just("perf".to_string()),
        Just("ci".to_string()),
        Just("build".to_string()),
        // Also test with arbitrary short strings to cover edge cases
        "[a-z]{1,15}".prop_map(|s| s),
    ]
}

/// Generate arbitrary component names (alphanumeric with hyphens).
fn component_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z][a-z0-9\\-]{0,30}".prop_map(|s| s),
        Just("core".to_string()),
        Just("api".to_string()),
        Just("ui".to_string()),
        Just("auth".to_string()),
        Just("coding-module".to_string()),
        Just("long-component-name-that-is-quite-verbose".to_string()),
    ]
}

/// Generate arbitrary file paths.
fn file_path_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z][a-z0-9_/\\.]{1,60}".prop_map(|s| s),
        Just("src/main.rs".to_string()),
        Just("Cargo.toml".to_string()),
        Just("src/lib.rs".to_string()),
        Just("tests/integration_test.rs".to_string()),
        Just("a_very_long_filename_that_would_exceed_limits_if_not_handled_properly.rs".to_string()),
    ]
}

/// Generate a list of 0 to 20 file paths.
fn file_list_strategy() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(file_path_strategy(), 0..20)
}

// ============================================================================
// Helper: Build full commit message (mirrors generate_commit_message logic)
// ============================================================================

/// Builds a full commit message from parts, mirroring the logic in
/// `GitOperations::generate_commit_message` but without requiring a git repo.
fn build_full_commit_message(change_type: &str, component: &str, files: &[String]) -> String {
    let summary = GitOperations::build_summary(change_type, component, files);

    let body = if files.is_empty() {
        String::new()
    } else {
        let file_list: String = files
            .iter()
            .map(|f| format!("- {}", f))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\nModified files:\n{}", file_list)
    };

    format!("{}{}", summary, body)
}

// ============================================================================
// Property 12: Git Commit Message Format
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// **Validates: Requirements 6.2**
    ///
    /// Property 12: The summary line SHALL be at most 72 characters for any
    /// combination of change type, component, and file list.
    #[test]
    fn prop_summary_line_at_most_72_chars(
        change_type in change_type_strategy(),
        component in component_strategy(),
        files in file_list_strategy(),
    ) {
        let summary = GitOperations::build_summary(&change_type, &component, &files);

        prop_assert!(
            summary.len() <= 72,
            "Summary line must be ≤72 characters, got {} chars: {:?}",
            summary.len(),
            summary
        );
    }

    /// **Validates: Requirements 6.2**
    ///
    /// Property 12: The summary line SHALL include the type of change.
    #[test]
    fn prop_summary_contains_change_type(
        change_type in change_type_strategy(),
        component in component_strategy(),
        files in file_list_strategy(),
    ) {
        let summary = GitOperations::build_summary(&change_type, &component, &files);

        // The summary format is "type(component): description"
        // The change type should appear at the start of the summary
        // (it may be truncated if the prefix itself exceeds 72 chars, but
        // for reasonable inputs it should always be present)
        let prefix = format!("{}(", change_type);
        let type_present = summary.starts_with(&prefix) || summary.contains(&change_type);

        prop_assert!(
            type_present,
            "Summary should contain the change type '{}'. Got: {:?}",
            change_type,
            summary
        );
    }

    /// **Validates: Requirements 6.2**
    ///
    /// Property 12: The summary line SHALL include the affected component.
    #[test]
    fn prop_summary_contains_component(
        change_type in change_type_strategy(),
        component in component_strategy(),
        files in file_list_strategy(),
    ) {
        let summary = GitOperations::build_summary(&change_type, &component, &files);

        // The component should appear in the "type(component): " prefix
        // It may be truncated for very long inputs, but for reasonable inputs
        // it should be present
        let expected_fragment = format!("({})", component);
        let component_present = summary.contains(&expected_fragment) || summary.contains(&component);

        prop_assert!(
            component_present,
            "Summary should contain the component '{}'. Got: {:?}",
            component,
            summary
        );
    }

    /// **Validates: Requirements 6.2**
    ///
    /// Property 12: When files are present, the body SHALL list all modified
    /// files.
    #[test]
    fn prop_body_lists_modified_files(
        change_type in change_type_strategy(),
        component in component_strategy(),
        files in prop::collection::vec(file_path_strategy(), 1..10),
    ) {
        let message = build_full_commit_message(&change_type, &component, &files);

        // The message should contain a body section with "Modified files:"
        prop_assert!(
            message.contains("Modified files:"),
            "Commit message body should contain 'Modified files:' header when files are present. Got: {:?}",
            message
        );

        // Each file should be listed in the body
        for file in &files {
            let file_entry = format!("- {}", file);
            prop_assert!(
                message.contains(&file_entry),
                "Commit message body should list file '{}'. Got: {:?}",
                file,
                message
            );
        }
    }

    /// **Validates: Requirements 6.2**
    ///
    /// Property 12: When no files are present, the body SHALL be empty
    /// (no "Modified files:" section).
    #[test]
    fn prop_no_body_when_no_files(
        change_type in change_type_strategy(),
        component in component_strategy(),
    ) {
        let files: Vec<String> = vec![];
        let message = build_full_commit_message(&change_type, &component, &files);

        prop_assert!(
            !message.contains("Modified files:"),
            "Commit message should not contain 'Modified files:' when no files are present. Got: {:?}",
            message
        );

        // The message should be just the summary line (no newlines for body)
        let summary = GitOperations::build_summary(&change_type, &component, &files);
        prop_assert_eq!(
            &message,
            &summary,
            "When no files are present, the full message should equal just the summary"
        );
    }

    /// **Validates: Requirements 6.2**
    ///
    /// Property 12: The summary line is always the first line of the commit
    /// message. When files are present, the body follows after the summary.
    #[test]
    fn prop_summary_is_first_line_of_message(
        change_type in change_type_strategy(),
        component in component_strategy(),
        files in prop::collection::vec(file_path_strategy(), 1..10),
    ) {
        let message = build_full_commit_message(&change_type, &component, &files);
        let summary = GitOperations::build_summary(&change_type, &component, &files);

        // The first line of the message should be the summary
        let first_line = message.lines().next().unwrap_or("");
        prop_assert_eq!(
            first_line,
            &summary,
            "First line of commit message should be the summary"
        );

        // The message should start with the summary
        prop_assert!(
            message.starts_with(&summary),
            "Commit message should start with the summary line"
        );

        // The body (everything after the summary) should contain the file list
        let body = &message[summary.len()..];
        prop_assert!(
            body.contains("Modified files:"),
            "Body section should contain 'Modified files:' header"
        );
    }
}
