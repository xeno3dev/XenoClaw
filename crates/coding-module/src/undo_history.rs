//! Undo History — tracks file operations and supports rollback.
//!
//! Provides:
//! - `UndoEntry` — records a single file operation with before/after state
//! - `UndoHistory` — bounded stack (max 50) of undo entries with rollback
//! - `TrackedFileOperations` — wraps `FileOperations` to automatically record
//!   each operation in the undo history

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::file_ops::FileOperations;

/// Default maximum number of undo entries to retain.
const DEFAULT_MAX_HISTORY: usize = 50;

// =============================================================================
// UndoEntry — records a single file operation
// =============================================================================

/// Represents a single undoable file operation.
#[derive(Debug, Clone, PartialEq)]
pub enum UndoEntry {
    /// A file was created. Undo by deleting it.
    Create {
        /// Path of the created file.
        path: PathBuf,
        /// Content that was written to the file.
        content: String,
    },
    /// A file was edited (content replaced). Undo by writing the before_content.
    Edit {
        /// Path of the edited file.
        path: PathBuf,
        /// Content before the edit.
        before_content: String,
        /// Content after the edit.
        after_content: String,
    },
    /// A file was deleted. Undo by recreating it with the original content.
    Delete {
        /// Path of the deleted file.
        path: PathBuf,
        /// Content the file had before deletion.
        content: String,
    },
}

impl UndoEntry {
    /// Returns the file path associated with this entry.
    pub fn path(&self) -> &Path {
        match self {
            UndoEntry::Create { path, .. } => path,
            UndoEntry::Edit { path, .. } => path,
            UndoEntry::Delete { path, .. } => path,
        }
    }

    /// Returns a human-readable description of the operation type.
    pub fn operation_type(&self) -> &'static str {
        match self {
            UndoEntry::Create { .. } => "create",
            UndoEntry::Edit { .. } => "edit",
            UndoEntry::Delete { .. } => "delete",
        }
    }
}

// =============================================================================
// UndoHistory — bounded stack of undo entries
// =============================================================================

/// A bounded stack of undo entries that supports rollback.
///
/// Maintains at most `capacity` entries. When full, the oldest entry is evicted
/// to make room for new ones.
#[derive(Debug)]
pub struct UndoHistory {
    /// The entries, ordered from oldest (front) to newest (back).
    entries: VecDeque<UndoEntry>,
    /// Maximum number of entries to retain.
    capacity: usize,
}

impl UndoHistory {
    /// Create a new UndoHistory with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Create a new UndoHistory with the default capacity (50).
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_MAX_HISTORY)
    }

    /// Record a new undo entry. Evicts the oldest entry if at capacity.
    pub fn record(&mut self, entry: UndoEntry) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// Returns the number of entries currently in the history.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the maximum capacity of the history.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns a slice view of all entries (oldest first).
    pub fn entries(&self) -> &VecDeque<UndoEntry> {
        &self.entries
    }

    /// Undo the last `count` operations in reverse order, restoring the filesystem.
    ///
    /// Returns a list of results for each undo operation attempted.
    /// Stops on the first error and returns what was accomplished so far.
    pub fn undo(&mut self, count: usize) -> Vec<UndoResult> {
        let actual_count = count.min(self.entries.len());
        let mut results = Vec::with_capacity(actual_count);

        for _ in 0..actual_count {
            let entry = match self.entries.pop_back() {
                Some(e) => e,
                None => break,
            };

            let result = Self::apply_undo(&entry);
            let success = result.is_ok();
            results.push(UndoResult {
                entry: entry.clone(),
                result,
            });

            if !success {
                // Put the entry back since we failed to undo it
                self.entries.push_back(entry);
                break;
            }
        }

        results
    }

    /// Apply a single undo operation to the filesystem.
    fn apply_undo(entry: &UndoEntry) -> Result<(), String> {
        match entry {
            UndoEntry::Create { path, .. } => {
                // Undo create by deleting the file
                std::fs::remove_file(path).map_err(|e| {
                    format!("Failed to undo create (delete '{}': {})", path.display(), e)
                })?;
                info!(path = %path.display(), "Undo: deleted created file");
                Ok(())
            }
            UndoEntry::Edit {
                path,
                before_content,
                ..
            } => {
                // Undo edit by restoring the before_content
                std::fs::write(path, before_content).map_err(|e| {
                    format!("Failed to undo edit (restore '{}': {})", path.display(), e)
                })?;
                info!(path = %path.display(), "Undo: restored file to previous content");
                Ok(())
            }
            UndoEntry::Delete { path, content } => {
                // Undo delete by recreating the file with original content
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        format!(
                            "Failed to undo delete (create dirs for '{}': {})",
                            path.display(),
                            e
                        )
                    })?;
                }
                std::fs::write(path, content).map_err(|e| {
                    format!(
                        "Failed to undo delete (recreate '{}': {})",
                        path.display(),
                        e
                    )
                })?;
                info!(path = %path.display(), "Undo: recreated deleted file");
                Ok(())
            }
        }
    }
}

/// Result of a single undo operation.
#[derive(Debug, Clone)]
pub struct UndoResult {
    /// The entry that was undone.
    pub entry: UndoEntry,
    /// Ok(()) if successful, Err(message) if failed.
    pub result: Result<(), String>,
}

// =============================================================================
// TrackedFileOperations — wraps FileOperations with undo tracking
// =============================================================================

/// Wraps `FileOperations` to automatically record each mutating operation
/// in an `UndoHistory`, enabling rollback.
#[derive(Debug)]
pub struct TrackedFileOperations {
    /// The underlying file operations implementation.
    ops: FileOperations,
    /// The undo history tracking all mutations.
    history: UndoHistory,
}

impl TrackedFileOperations {
    /// Create a new TrackedFileOperations wrapping the given FileOperations.
    pub fn new(ops: FileOperations) -> Self {
        Self {
            ops,
            history: UndoHistory::with_default_capacity(),
        }
    }

    /// Create a new TrackedFileOperations with a custom history capacity.
    pub fn with_capacity(ops: FileOperations, capacity: usize) -> Self {
        Self {
            ops,
            history: UndoHistory::new(capacity),
        }
    }

    /// Get a reference to the underlying FileOperations.
    pub fn ops(&self) -> &FileOperations {
        &self.ops
    }

    /// Get a reference to the undo history.
    pub fn history(&self) -> &UndoHistory {
        &self.history
    }

    /// Read a file (not tracked — reads don't mutate).
    pub fn read_file(&self, path: &Path) -> Result<String, String> {
        self.ops.read_file(path)
    }

    /// Create a new file and record the operation in undo history.
    pub fn create_file(&mut self, path: &Path, content: &str) -> Result<(), String> {
        self.ops.create_file(path, content)?;

        // Record the create — we need the canonical path for undo
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.history.record(UndoEntry::Create {
            path: canonical,
            content: content.to_string(),
        });

        Ok(())
    }

    /// Write (overwrite) a file and record the operation in undo history.
    ///
    /// This is treated as an edit since it replaces existing content.
    pub fn write_file(&mut self, path: &Path, content: &str) -> Result<(), String> {
        // Capture before-state
        let before_content = self.ops.read_file(path)?;

        self.ops.write_file(path, content)?;

        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.history.record(UndoEntry::Edit {
            path: canonical,
            before_content,
            after_content: content.to_string(),
        });

        Ok(())
    }

    /// Edit a file using string replacement and record the operation in undo history.
    pub fn edit_file(
        &mut self,
        path: &Path,
        old_str: &str,
        new_str: &str,
    ) -> Result<String, String> {
        // Capture before-state
        let before_content = self.ops.read_file(path)?;

        let after_content = self.ops.edit_file(path, old_str, new_str)?;

        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.history.record(UndoEntry::Edit {
            path: canonical,
            before_content,
            after_content: after_content.clone(),
        });

        Ok(after_content)
    }

    /// Delete a file and record the operation in undo history.
    pub fn delete_file(&mut self, path: &Path) -> Result<(), String> {
        // Capture content before deletion
        let content = self.ops.read_file(path)?;
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        self.ops.delete_file(path)?;

        self.history.record(UndoEntry::Delete {
            path: canonical,
            content,
        });

        Ok(())
    }

    /// Undo the last `count` operations, restoring the filesystem.
    ///
    /// Returns a list of undo results for each operation rolled back.
    pub fn undo(&mut self, count: usize) -> Vec<UndoResult> {
        let results = self.history.undo(count);
        if !results.is_empty() {
            let successful = results.iter().filter(|r| r.result.is_ok()).count();
            let failed = results.iter().filter(|r| r.result.is_err()).count();
            info!(
                successful = successful,
                failed = failed,
                "Undo completed: {} operations rolled back, {} failed",
                successful,
                failed
            );
        }
        results
    }

    /// Returns the number of operations in the undo history.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    use common::config::FilesystemRule;

    /// Helper to create a TrackedFileOperations with a temp directory as workspace.
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

    // --- UndoHistory unit tests ---

    #[test]
    fn test_undo_history_capacity() {
        let mut history = UndoHistory::new(3);
        assert_eq!(history.capacity(), 3);
        assert_eq!(history.len(), 0);
        assert!(history.is_empty());

        // Add 3 entries
        history.record(UndoEntry::Create {
            path: PathBuf::from("/a"),
            content: "a".to_string(),
        });
        history.record(UndoEntry::Create {
            path: PathBuf::from("/b"),
            content: "b".to_string(),
        });
        history.record(UndoEntry::Create {
            path: PathBuf::from("/c"),
            content: "c".to_string(),
        });
        assert_eq!(history.len(), 3);

        // Adding a 4th should evict the oldest
        history.record(UndoEntry::Create {
            path: PathBuf::from("/d"),
            content: "d".to_string(),
        });
        assert_eq!(history.len(), 3);

        // The oldest (/a) should be gone, newest entries are /b, /c, /d
        let paths: Vec<_> = history
            .entries()
            .iter()
            .map(|e| e.path().to_path_buf())
            .collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/b"),
                PathBuf::from("/c"),
                PathBuf::from("/d"),
            ]
        );
    }

    #[test]
    fn test_undo_history_default_capacity() {
        let history = UndoHistory::with_default_capacity();
        assert_eq!(history.capacity(), 50);
    }

    #[test]
    fn test_undo_entry_accessors() {
        let create = UndoEntry::Create {
            path: PathBuf::from("/foo/bar.txt"),
            content: "hello".to_string(),
        };
        assert_eq!(create.path(), Path::new("/foo/bar.txt"));
        assert_eq!(create.operation_type(), "create");

        let edit = UndoEntry::Edit {
            path: PathBuf::from("/baz.txt"),
            before_content: "old".to_string(),
            after_content: "new".to_string(),
        };
        assert_eq!(edit.path(), Path::new("/baz.txt"));
        assert_eq!(edit.operation_type(), "edit");

        let delete = UndoEntry::Delete {
            path: PathBuf::from("/gone.txt"),
            content: "was here".to_string(),
        };
        assert_eq!(delete.path(), Path::new("/gone.txt"));
        assert_eq!(delete.operation_type(), "delete");
    }

    // --- TrackedFileOperations integration tests ---

    #[test]
    fn test_tracked_create_and_undo() {
        let (tmp, mut tracked) = setup();
        let file_path = tmp.path().join("created.txt");

        // Create a file
        tracked.create_file(&file_path, "hello").unwrap();
        assert!(file_path.exists());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "hello");
        assert_eq!(tracked.history_len(), 1);

        // Undo the create (should delete the file)
        let results = tracked.undo(1);
        assert_eq!(results.len(), 1);
        assert!(results[0].result.is_ok());
        assert!(!file_path.exists());
        assert_eq!(tracked.history_len(), 0);
    }

    #[test]
    fn test_tracked_edit_and_undo() {
        let (tmp, mut tracked) = setup();
        let file_path = tmp.path().join("edit_me.txt");
        fs::write(&file_path, "original content").unwrap();

        // Edit the file
        let result = tracked
            .edit_file(&file_path, "original", "modified")
            .unwrap();
        assert_eq!(result, "modified content");
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "modified content");
        assert_eq!(tracked.history_len(), 1);

        // Undo the edit (should restore original content)
        let results = tracked.undo(1);
        assert_eq!(results.len(), 1);
        assert!(results[0].result.is_ok());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "original content");
        assert_eq!(tracked.history_len(), 0);
    }

    #[test]
    fn test_tracked_write_and_undo() {
        let (tmp, mut tracked) = setup();
        let file_path = tmp.path().join("write_me.txt");
        fs::write(&file_path, "before").unwrap();

        // Write (overwrite) the file
        tracked.write_file(&file_path, "after").unwrap();
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "after");
        assert_eq!(tracked.history_len(), 1);

        // Undo the write (should restore "before")
        let results = tracked.undo(1);
        assert_eq!(results.len(), 1);
        assert!(results[0].result.is_ok());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "before");
    }

    #[test]
    fn test_tracked_delete_and_undo() {
        let (tmp, mut tracked) = setup();
        let file_path = tmp.path().join("delete_me.txt");
        fs::write(&file_path, "precious data").unwrap();

        // Delete the file
        tracked.delete_file(&file_path).unwrap();
        assert!(!file_path.exists());
        assert_eq!(tracked.history_len(), 1);

        // Undo the delete (should recreate the file)
        let results = tracked.undo(1);
        assert_eq!(results.len(), 1);
        assert!(results[0].result.is_ok());
        assert!(file_path.exists());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "precious data");
    }

    #[test]
    fn test_tracked_multiple_operations_undo_all() {
        let (tmp, mut tracked) = setup();

        let file_a = tmp.path().join("a.txt");
        let file_b = tmp.path().join("b.txt");
        let file_c = tmp.path().join("c.txt");

        // Create file_a
        tracked.create_file(&file_a, "aaa").unwrap();
        // Create file_b
        tracked.create_file(&file_b, "bbb").unwrap();
        // Edit file_a
        tracked.edit_file(&file_a, "aaa", "AAA").unwrap();
        // Create file_c
        tracked.create_file(&file_c, "ccc").unwrap();

        assert_eq!(tracked.history_len(), 4);
        assert_eq!(fs::read_to_string(&file_a).unwrap(), "AAA");
        assert!(file_b.exists());
        assert!(file_c.exists());

        // Undo all 4 operations in reverse order:
        // 1. Undo create file_c → delete file_c
        // 2. Undo edit file_a → restore file_a to "aaa"
        // 3. Undo create file_b → delete file_b
        // 4. Undo create file_a → delete file_a
        let results = tracked.undo(4);
        assert_eq!(results.len(), 4);
        assert!(results.iter().all(|r| r.result.is_ok()));

        // After all undos, all files should be gone (filesystem restored to initial state)
        assert!(!file_c.exists());
        assert!(!file_b.exists());
        assert!(!file_a.exists());
    }

    #[test]
    fn test_tracked_undo_more_than_available() {
        let (tmp, mut tracked) = setup();
        let file_path = tmp.path().join("only_one.txt");

        tracked.create_file(&file_path, "content").unwrap();
        assert_eq!(tracked.history_len(), 1);

        // Request undo of 10, but only 1 is available
        let results = tracked.undo(10);
        assert_eq!(results.len(), 1);
        assert!(results[0].result.is_ok());
        assert!(!file_path.exists());
        assert_eq!(tracked.history_len(), 0);
    }

    #[test]
    fn test_tracked_undo_zero() {
        let (tmp, mut tracked) = setup();
        let file_path = tmp.path().join("no_undo.txt");

        tracked.create_file(&file_path, "content").unwrap();

        // Undo 0 operations — nothing should happen
        let results = tracked.undo(0);
        assert!(results.is_empty());
        assert!(file_path.exists());
        assert_eq!(tracked.history_len(), 1);
    }

    #[test]
    fn test_tracked_read_does_not_record() {
        let (tmp, mut tracked) = setup();
        let file_path = tmp.path().join("readable.txt");
        fs::write(&file_path, "read me").unwrap();

        let content = tracked.read_file(&file_path).unwrap();
        assert_eq!(content, "read me");
        assert_eq!(tracked.history_len(), 0);
    }

    #[test]
    fn test_tracked_capacity_eviction() {
        let (tmp, _) = setup();
        let rules = vec![FilesystemRule {
            path: tmp.path().to_path_buf(),
            read: true,
            write: true,
            execute: false,
        }];
        let ops = FileOperations::new(rules);
        let mut tracked = TrackedFileOperations::with_capacity(ops, 3);

        // Create 4 files — the first should be evicted from history
        for i in 0..4 {
            let path = tmp.path().join(format!("file_{}.txt", i));
            tracked
                .create_file(&path, &format!("content {}", i))
                .unwrap();
        }

        assert_eq!(tracked.history_len(), 3);

        // Undo all 3 remaining — files 1, 2, 3 should be deleted
        let results = tracked.undo(3);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.result.is_ok()));

        // file_0 still exists (its undo entry was evicted)
        assert!(tmp.path().join("file_0.txt").exists());
        // files 1, 2, 3 should be gone
        assert!(!tmp.path().join("file_1.txt").exists());
        assert!(!tmp.path().join("file_2.txt").exists());
        assert!(!tmp.path().join("file_3.txt").exists());
    }

    #[test]
    fn test_tracked_delete_in_subdirectory_undo_recreates() {
        let (tmp, mut tracked) = setup();
        let sub_dir = tmp.path().join("sub").join("dir");
        fs::create_dir_all(&sub_dir).unwrap();
        let file_path = sub_dir.join("nested.txt");
        fs::write(&file_path, "nested content").unwrap();

        // Delete the file
        tracked.delete_file(&file_path).unwrap();
        assert!(!file_path.exists());

        // Remove the parent directories too
        fs::remove_dir_all(tmp.path().join("sub")).unwrap();

        // Undo should recreate the file (and parent dirs)
        let results = tracked.undo(1);
        assert_eq!(results.len(), 1);
        assert!(results[0].result.is_ok());
        assert!(file_path.exists());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "nested content");
    }
}
