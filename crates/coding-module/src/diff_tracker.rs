//! Diff Tracker — tracks file modifications during a session and generates unified diffs.
//!
//! Provides:
//! - `ChangeTracker` — tracks all file modifications during a session
//! - `UnifiedDiff` — represents a unified diff for a single file
//! - `DiffHunk` — a single hunk within a unified diff
//! - `SessionChangeSummary` — summary of all changes in a session
//!
//! Uses the `similar` crate for diff computation.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use similar::{ChangeTag, TextDiff};
use tracing::info;

/// Threshold for detecting binary content.
/// If a file contains a null byte in the first 8KB, it's considered binary.
const BINARY_CHECK_SIZE: usize = 8192;

// =============================================================================
// FileChange — records a single modification
// =============================================================================

/// Records a single file modification with before/after states.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// The file path that was modified.
    pub path: PathBuf,
    /// Content before the modification (None for binary files or new files).
    pub before_content: Option<String>,
    /// Content after the modification (None for binary files or deleted files).
    pub after_content: Option<String>,
    /// Whether this file is binary (non-text).
    pub is_binary: bool,
    /// Timestamp of the modification.
    pub timestamp: DateTime<Utc>,
}

// =============================================================================
// DiffHunk — a single hunk in a unified diff
// =============================================================================

/// A single hunk in a unified diff, representing a contiguous region of changes.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffHunk {
    /// Starting line number in the original file (1-based).
    pub old_start: usize,
    /// Number of lines from the original file in this hunk.
    pub old_count: usize,
    /// Starting line number in the new file (1-based).
    pub new_start: usize,
    /// Number of lines from the new file in this hunk.
    pub new_count: usize,
    /// The lines in this hunk (context, additions, removals).
    pub lines: Vec<DiffLine>,
}

/// A single line in a diff hunk.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffLine {
    /// A context line (unchanged).
    Context(String),
    /// An added line.
    Addition(String),
    /// A removed line.
    Removal(String),
}

impl fmt::Display for DiffLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiffLine::Context(line) => write!(f, " {}", line),
            DiffLine::Addition(line) => write!(f, "+{}", line),
            DiffLine::Removal(line) => write!(f, "-{}", line),
        }
    }
}

// =============================================================================
// UnifiedDiff — represents a complete unified diff for a file
// =============================================================================

/// A unified diff for a single file.
#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedDiff {
    /// Path of the original file.
    pub old_path: PathBuf,
    /// Path of the new file.
    pub new_path: PathBuf,
    /// Whether this is a binary file change.
    pub is_binary: bool,
    /// The hunks making up this diff (empty for binary files).
    pub hunks: Vec<DiffHunk>,
    /// Total lines added.
    pub lines_added: usize,
    /// Total lines removed.
    pub lines_removed: usize,
}

impl fmt::Display for UnifiedDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_binary {
            writeln!(f, "Binary file {} changed", self.new_path.display())?;
            return Ok(());
        }

        writeln!(f, "--- {}", self.old_path.display())?;
        writeln!(f, "+++ {}", self.new_path.display())?;

        for hunk in &self.hunks {
            writeln!(
                f,
                "@@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
            )?;
            for line in &hunk.lines {
                writeln!(f, "{}", line)?;
            }
        }

        Ok(())
    }
}

// =============================================================================
// SessionChangeSummary — summary of all changes in a session
// =============================================================================

/// Summary of all changes made during a session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionChangeSummary {
    /// All file paths that were modified.
    pub modified_files: Vec<PathBuf>,
    /// Total lines added across all files.
    pub total_lines_added: usize,
    /// Total lines removed across all files.
    pub total_lines_removed: usize,
}

// =============================================================================
// ChangeTracker — tracks all file modifications during a session
// =============================================================================

/// Tracks all file modifications during a session and generates diffs.
///
/// For each file, stores the original state (before the first modification)
/// and the current state (after the most recent modification). This allows
/// generating cumulative diffs that show the total effect of all changes.
#[derive(Debug)]
pub struct ChangeTracker {
    /// Maps file path to (original_content, current_content, is_binary).
    /// original_content is None for files that didn't exist before.
    /// current_content is None for files that were deleted.
    file_states: HashMap<PathBuf, FileState>,
    /// Ordered list of all modifications for history.
    modifications: Vec<FileChange>,
    /// Number of context lines to include in diffs.
    context_lines: usize,
}

/// Internal state for a tracked file.
#[derive(Debug, Clone)]
struct FileState {
    /// Content before the first modification in this session.
    original_content: Option<String>,
    /// Content after the most recent modification.
    current_content: Option<String>,
    /// Whether this file is binary.
    is_binary: bool,
}

impl ChangeTracker {
    /// Create a new ChangeTracker with the default context lines (3).
    pub fn new() -> Self {
        Self {
            file_states: HashMap::new(),
            modifications: Vec::new(),
            context_lines: 3,
        }
    }

    /// Create a new ChangeTracker with a custom number of context lines.
    pub fn with_context_lines(context_lines: usize) -> Self {
        Self {
            file_states: HashMap::new(),
            modifications: Vec::new(),
            context_lines,
        }
    }

    /// Record a file modification.
    ///
    /// - `path`: The file that was modified.
    /// - `before_content`: Content before the modification (None for new files).
    /// - `after_content`: Content after the modification (None for deleted files).
    pub fn record_change(
        &mut self,
        path: &Path,
        before_content: Option<&str>,
        after_content: Option<&str>,
    ) {
        let is_binary = before_content.map_or(false, |c| is_binary_content(c))
            || after_content.map_or(false, |c| is_binary_content(c));

        let canonical = path.to_path_buf();

        // Update file state: store original only on first modification
        let state = self
            .file_states
            .entry(canonical.clone())
            .or_insert_with(|| FileState {
                original_content: before_content.map(|s| s.to_string()),
                current_content: None,
                is_binary,
            });

        // Always update the current content and binary flag
        state.current_content = after_content.map(|s| s.to_string());
        state.is_binary = state.is_binary || is_binary;

        // Record the individual modification
        self.modifications.push(FileChange {
            path: canonical,
            before_content: before_content.map(|s| s.to_string()),
            after_content: after_content.map(|s| s.to_string()),
            is_binary,
            timestamp: Utc::now(),
        });

        info!(
            path = %path.display(),
            is_binary = is_binary,
            "Recorded file change"
        );
    }

    /// Get the cumulative unified diff for a specific file.
    ///
    /// Returns the diff between the original state (before first modification)
    /// and the current state (after most recent modification).
    /// Returns None if the file has not been modified.
    pub fn get_cumulative_diff(&self, path: &Path) -> Option<UnifiedDiff> {
        let state = self.file_states.get(path)?;

        if state.is_binary {
            return Some(UnifiedDiff {
                old_path: path.to_path_buf(),
                new_path: path.to_path_buf(),
                is_binary: true,
                hunks: Vec::new(),
                lines_added: 0,
                lines_removed: 0,
            });
        }

        let old_content = state.original_content.as_deref().unwrap_or("");
        let new_content = state.current_content.as_deref().unwrap_or("");

        Some(generate_unified_diff(
            path,
            path,
            old_content,
            new_content,
            self.context_lines,
        ))
    }

    /// Get cumulative diffs for all modified files.
    pub fn get_all_diffs(&self) -> Vec<UnifiedDiff> {
        self.file_states
            .keys()
            .filter_map(|path| self.get_cumulative_diff(path))
            .collect()
    }

    /// Generate a session change summary.
    ///
    /// Returns a summary listing all modified files and total lines added/removed.
    pub fn session_summary(&self) -> SessionChangeSummary {
        let mut total_added = 0;
        let mut total_removed = 0;
        let mut modified_files = Vec::new();

        for (path, state) in &self.file_states {
            modified_files.push(path.clone());

            if state.is_binary {
                // Binary files don't contribute to line counts
                continue;
            }

            let old_content = state.original_content.as_deref().unwrap_or("");
            let new_content = state.current_content.as_deref().unwrap_or("");

            let diff = TextDiff::from_lines(old_content, new_content);
            for change in diff.iter_all_changes() {
                match change.tag() {
                    ChangeTag::Insert => total_added += 1,
                    ChangeTag::Delete => total_removed += 1,
                    ChangeTag::Equal => {}
                }
            }
        }

        modified_files.sort();

        SessionChangeSummary {
            modified_files,
            total_lines_added: total_added,
            total_lines_removed: total_removed,
        }
    }

    /// Reset all tracking for a new session.
    pub fn clear(&mut self) {
        self.file_states.clear();
        self.modifications.clear();
        info!("Change tracker cleared for new session");
    }

    /// Returns the number of files currently being tracked.
    pub fn tracked_file_count(&self) -> usize {
        self.file_states.len()
    }

    /// Returns the total number of individual modifications recorded.
    pub fn modification_count(&self) -> usize {
        self.modifications.len()
    }

    /// Returns a reference to all recorded modifications.
    pub fn modifications(&self) -> &[FileChange] {
        &self.modifications
    }
}

impl Default for ChangeTracker {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Helper functions
// =============================================================================

/// Detect whether content is binary (non-text).
///
/// A file is considered binary if it contains a null byte within the first
/// BINARY_CHECK_SIZE bytes.
pub fn is_binary_content(content: &str) -> bool {
    let check_len = content.len().min(BINARY_CHECK_SIZE);
    content.as_bytes()[..check_len].contains(&0)
}

/// Generate a unified diff between two text contents.
///
/// Produces standard unified diff format with the specified number of context lines.
pub fn generate_unified_diff(
    old_path: &Path,
    new_path: &Path,
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> UnifiedDiff {
    let text_diff = TextDiff::from_lines(old_content, new_content);

    let mut hunks = Vec::new();
    let mut total_added = 0;
    let mut total_removed = 0;

    // Use similar's unified diff groups to get hunks with context
    for group in text_diff.grouped_ops(context_lines) {
        let mut hunk_lines = Vec::new();
        let mut old_start = 0;
        let mut old_count = 0;
        let mut new_start = 0;
        let mut new_count = 0;
        let mut first = true;

        for op in &group {
            if first {
                // Line numbers are 1-based in unified diff format
                old_start = op.old_range().start + 1;
                new_start = op.new_range().start + 1;
                first = false;
            }

            for change in text_diff.iter_changes(op) {
                let line_content = change.value().trim_end_matches('\n').to_string();
                match change.tag() {
                    ChangeTag::Equal => {
                        hunk_lines.push(DiffLine::Context(line_content));
                        old_count += 1;
                        new_count += 1;
                    }
                    ChangeTag::Insert => {
                        hunk_lines.push(DiffLine::Addition(line_content));
                        new_count += 1;
                        total_added += 1;
                    }
                    ChangeTag::Delete => {
                        hunk_lines.push(DiffLine::Removal(line_content));
                        old_count += 1;
                        total_removed += 1;
                    }
                }
            }
        }

        // Handle edge case: if old_start or new_start is 0 (empty file),
        // unified diff convention uses 0,0
        if old_content.is_empty() && old_start == 1 {
            old_start = 0;
            old_count = 0;
        }
        if new_content.is_empty() && new_start == 1 {
            new_start = 0;
            new_count = 0;
        }

        hunks.push(DiffHunk {
            old_start,
            old_count,
            new_start,
            new_count,
            lines: hunk_lines,
        });
    }

    UnifiedDiff {
        old_path: old_path.to_path_buf(),
        new_path: new_path.to_path_buf(),
        is_binary: false,
        hunks,
        lines_added: total_added,
        lines_removed: total_removed,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Binary detection tests ---

    #[test]
    fn test_text_content_not_binary() {
        assert!(!is_binary_content("Hello, world!\nThis is text.\n"));
    }

    #[test]
    fn test_binary_content_detected() {
        let content = "Hello\x00World";
        assert!(is_binary_content(content));
    }

    #[test]
    fn test_empty_content_not_binary() {
        assert!(!is_binary_content(""));
    }

    // --- UnifiedDiff generation tests ---

    #[test]
    fn test_simple_addition() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nline2\nnew_line\nline3\n";
        let diff = generate_unified_diff(Path::new("a.txt"), Path::new("a.txt"), old, new, 3);

        assert_eq!(diff.lines_added, 1);
        assert_eq!(diff.lines_removed, 0);
        assert!(!diff.is_binary);
        assert!(!diff.hunks.is_empty());
    }

    #[test]
    fn test_simple_removal() {
        let old = "line1\nline2\nline3\nline4\n";
        let new = "line1\nline3\nline4\n";
        let diff = generate_unified_diff(Path::new("a.txt"), Path::new("a.txt"), old, new, 3);

        assert_eq!(diff.lines_added, 0);
        assert_eq!(diff.lines_removed, 1);
        assert!(!diff.hunks.is_empty());
    }

    #[test]
    fn test_modification() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nmodified\nline3\n";
        let diff = generate_unified_diff(Path::new("a.txt"), Path::new("a.txt"), old, new, 3);

        assert_eq!(diff.lines_added, 1);
        assert_eq!(diff.lines_removed, 1);
    }

    #[test]
    fn test_no_changes() {
        let content = "line1\nline2\nline3\n";
        let diff =
            generate_unified_diff(Path::new("a.txt"), Path::new("a.txt"), content, content, 3);

        assert_eq!(diff.lines_added, 0);
        assert_eq!(diff.lines_removed, 0);
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn test_context_lines_present() {
        // Create a file with many lines, change one in the middle
        let old_lines: Vec<String> = (1..=20).map(|i| format!("line{}", i)).collect();
        let mut new_lines = old_lines.clone();
        new_lines[9] = "CHANGED".to_string(); // Change line 10

        let old = old_lines.join("\n") + "\n";
        let new = new_lines.join("\n") + "\n";

        let diff = generate_unified_diff(Path::new("a.txt"), Path::new("a.txt"), &old, &new, 3);

        assert_eq!(diff.hunks.len(), 1);
        let hunk = &diff.hunks[0];

        // Should have context lines before and after the change
        // 3 context before + 1 removal + 1 addition + 3 context after = 8 lines
        let context_count = hunk
            .lines
            .iter()
            .filter(|l| matches!(l, DiffLine::Context(_)))
            .count();
        assert!(
            context_count >= 3,
            "Should have at least 3 context lines, got {}",
            context_count
        );
    }

    #[test]
    fn test_diff_display_format() {
        let old = "hello\nworld\n";
        let new = "hello\nearth\n";
        let diff = generate_unified_diff(Path::new("test.txt"), Path::new("test.txt"), old, new, 3);

        let output = diff.to_string();
        assert!(output.contains("--- test.txt"));
        assert!(output.contains("+++ test.txt"));
        assert!(output.contains("@@"));
        assert!(output.contains("-world"));
        assert!(output.contains("+earth"));
    }

    #[test]
    fn test_binary_diff_display() {
        let diff = UnifiedDiff {
            old_path: PathBuf::from("image.png"),
            new_path: PathBuf::from("image.png"),
            is_binary: true,
            hunks: Vec::new(),
            lines_added: 0,
            lines_removed: 0,
        };

        let output = diff.to_string();
        assert!(output.contains("Binary file image.png changed"));
    }

    #[test]
    fn test_new_file_diff() {
        let diff = generate_unified_diff(
            Path::new("new.txt"),
            Path::new("new.txt"),
            "",
            "line1\nline2\nline3\n",
            3,
        );

        assert_eq!(diff.lines_added, 3);
        assert_eq!(diff.lines_removed, 0);
    }

    #[test]
    fn test_deleted_file_diff() {
        let diff = generate_unified_diff(
            Path::new("old.txt"),
            Path::new("old.txt"),
            "line1\nline2\nline3\n",
            "",
            3,
        );

        assert_eq!(diff.lines_added, 0);
        assert_eq!(diff.lines_removed, 3);
    }

    // --- ChangeTracker tests ---

    #[test]
    fn test_tracker_record_single_change() {
        let mut tracker = ChangeTracker::new();
        tracker.record_change(
            Path::new("/tmp/test.txt"),
            Some("old content\n"),
            Some("new content\n"),
        );

        assert_eq!(tracker.tracked_file_count(), 1);
        assert_eq!(tracker.modification_count(), 1);
    }

    #[test]
    fn test_tracker_cumulative_diff() {
        let mut tracker = ChangeTracker::new();
        let path = Path::new("/tmp/test.txt");

        // First modification: "aaa\n" -> "bbb\n"
        tracker.record_change(path, Some("aaa\n"), Some("bbb\n"));
        // Second modification: "bbb\n" -> "ccc\n"
        tracker.record_change(path, Some("bbb\n"), Some("ccc\n"));

        // Cumulative diff should be "aaa\n" -> "ccc\n"
        let diff = tracker.get_cumulative_diff(path).unwrap();
        assert_eq!(diff.lines_removed, 1); // "aaa" removed
        assert_eq!(diff.lines_added, 1); // "ccc" added

        // Verify the original state is preserved
        assert_eq!(tracker.tracked_file_count(), 1);
        assert_eq!(tracker.modification_count(), 2);
    }

    #[test]
    fn test_tracker_multiple_files() {
        let mut tracker = ChangeTracker::new();

        tracker.record_change(Path::new("/tmp/a.txt"), Some("old_a\n"), Some("new_a\n"));
        tracker.record_change(Path::new("/tmp/b.txt"), Some("old_b\n"), Some("new_b\n"));

        assert_eq!(tracker.tracked_file_count(), 2);
        let diffs = tracker.get_all_diffs();
        assert_eq!(diffs.len(), 2);
    }

    #[test]
    fn test_tracker_binary_file() {
        let mut tracker = ChangeTracker::new();
        let path = Path::new("/tmp/image.png");

        // Content with null byte = binary
        tracker.record_change(path, Some("old\x00data"), Some("new\x00data"));

        let diff = tracker.get_cumulative_diff(path).unwrap();
        assert!(diff.is_binary);
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn test_tracker_session_summary() {
        let mut tracker = ChangeTracker::new();

        // File a: add 2 lines
        tracker.record_change(
            Path::new("/tmp/a.txt"),
            Some("line1\n"),
            Some("line1\nline2\nline3\n"),
        );
        // File b: remove 1 line, add 1 line
        tracker.record_change(Path::new("/tmp/b.txt"), Some("old\n"), Some("new\n"));

        let summary = tracker.session_summary();
        assert_eq!(summary.modified_files.len(), 2);
        assert_eq!(summary.total_lines_added, 3); // 2 from a + 1 from b
        assert_eq!(summary.total_lines_removed, 1); // 1 from b
        assert!(summary
            .modified_files
            .contains(&PathBuf::from("/tmp/a.txt")));
        assert!(summary
            .modified_files
            .contains(&PathBuf::from("/tmp/b.txt")));
    }

    #[test]
    fn test_tracker_session_summary_with_binary() {
        let mut tracker = ChangeTracker::new();

        tracker.record_change(Path::new("/tmp/text.txt"), Some("old\n"), Some("new\n"));
        tracker.record_change(
            Path::new("/tmp/bin.dat"),
            Some("data\x00here"),
            Some("new\x00data"),
        );

        let summary = tracker.session_summary();
        assert_eq!(summary.modified_files.len(), 2);
        // Binary files don't contribute to line counts
        assert_eq!(summary.total_lines_added, 1);
        assert_eq!(summary.total_lines_removed, 1);
    }

    #[test]
    fn test_tracker_clear() {
        let mut tracker = ChangeTracker::new();
        tracker.record_change(Path::new("/tmp/test.txt"), Some("old\n"), Some("new\n"));

        assert_eq!(tracker.tracked_file_count(), 1);
        tracker.clear();
        assert_eq!(tracker.tracked_file_count(), 0);
        assert_eq!(tracker.modification_count(), 0);
    }

    #[test]
    fn test_tracker_new_file_creation() {
        let mut tracker = ChangeTracker::new();
        let path = Path::new("/tmp/new.txt");

        // before_content is None for a new file
        tracker.record_change(path, None, Some("new content\n"));

        let diff = tracker.get_cumulative_diff(path).unwrap();
        assert_eq!(diff.lines_added, 1);
        assert_eq!(diff.lines_removed, 0);
    }

    #[test]
    fn test_tracker_file_deletion() {
        let mut tracker = ChangeTracker::new();
        let path = Path::new("/tmp/deleted.txt");

        // after_content is None for a deleted file
        tracker.record_change(path, Some("content\n"), None);

        let diff = tracker.get_cumulative_diff(path).unwrap();
        assert_eq!(diff.lines_added, 0);
        assert_eq!(diff.lines_removed, 1);
    }

    #[test]
    fn test_tracker_get_diff_untracked_file() {
        let tracker = ChangeTracker::new();
        let result = tracker.get_cumulative_diff(Path::new("/tmp/unknown.txt"));
        assert!(result.is_none());
    }

    #[test]
    fn test_tracker_cumulative_preserves_original() {
        let mut tracker = ChangeTracker::new();
        let path = Path::new("/tmp/multi.txt");

        // Original: "A\nB\nC\n"
        // First edit: "A\nB\nC\n" -> "A\nX\nC\n"
        tracker.record_change(path, Some("A\nB\nC\n"), Some("A\nX\nC\n"));
        // Second edit: "A\nX\nC\n" -> "A\nX\nY\n"
        tracker.record_change(path, Some("A\nX\nC\n"), Some("A\nX\nY\n"));
        // Third edit: "A\nX\nY\n" -> "A\nX\nY\nZ\n"
        tracker.record_change(path, Some("A\nX\nY\n"), Some("A\nX\nY\nZ\n"));

        // Cumulative diff should be "A\nB\nC\n" -> "A\nX\nY\nZ\n"
        let diff = tracker.get_cumulative_diff(path).unwrap();
        // B and C removed (2), X, Y, Z added (3)
        assert_eq!(diff.lines_removed, 2);
        assert_eq!(diff.lines_added, 3);
    }

    #[test]
    fn test_default_context_lines() {
        let tracker = ChangeTracker::new();
        assert_eq!(tracker.context_lines, 3);
    }

    #[test]
    fn test_custom_context_lines() {
        let tracker = ChangeTracker::with_context_lines(5);
        assert_eq!(tracker.context_lines, 5);
    }
}
