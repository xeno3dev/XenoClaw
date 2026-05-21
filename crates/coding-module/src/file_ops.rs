//! File Operations — read, create, write, edit, and delete files.
//!
//! Provides a `FileOperations` struct that holds sandbox config and implements
//! the underlying operations. Individual tool structs delegate to it.
//!
//! All operations:
//! - Validate paths through the Security Layer sandbox
//! - Enforce a 10MB file size limit
//! - Return appropriate error messages

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use agent_core::Tool;
use common::config::FilesystemRule;
use common::models::AccessType;
use security_layer::validate_path;

/// Maximum file size in bytes (10MB).
const MAX_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024;

// =============================================================================
// FileOperations — shared implementation struct
// =============================================================================

/// Holds sandbox configuration and provides the underlying file operations.
///
/// This struct is designed to be shared (via Arc) between the tool implementations
/// and will be reused by the undo system (task 12.3).
#[derive(Debug, Clone)]
pub struct FileOperations {
    /// Filesystem rules for sandbox validation.
    pub filesystem_rules: Vec<FilesystemRule>,
    /// Maximum file size in bytes.
    pub max_file_size: u64,
}

impl FileOperations {
    /// Create a new FileOperations with the given sandbox rules.
    pub fn new(filesystem_rules: Vec<FilesystemRule>) -> Self {
        Self {
            filesystem_rules,
            max_file_size: MAX_FILE_SIZE_BYTES,
        }
    }

    /// Validate a path for the given access type through the security sandbox.
    fn validate(&self, path: &Path, access: AccessType) -> Result<PathBuf, String> {
        validate_path(path, access, &self.filesystem_rules).map_err(|e| {
            warn!(path = %path.display(), "Path validation failed: {}", e);
            format!(
                "Access denied: path '{}' is outside the allowed workspace directories",
                path.display()
            )
        })
    }

    /// Check that a file's size does not exceed the limit.
    fn check_file_size(&self, path: &Path) -> Result<(), String> {
        match std::fs::metadata(path) {
            Ok(meta) => {
                if meta.len() > self.max_file_size {
                    Err(format!(
                        "File size limit exceeded: '{}' is {} bytes (limit: {} bytes)",
                        path.display(),
                        meta.len(),
                        self.max_file_size
                    ))
                } else {
                    Ok(())
                }
            }
            Err(_) => Ok(()), // File doesn't exist yet, that's fine
        }
    }

    /// Check that content size does not exceed the limit.
    fn check_content_size(&self, content: &str) -> Result<(), String> {
        if content.len() as u64 > self.max_file_size {
            Err(format!(
                "Content size limit exceeded: {} bytes (limit: {} bytes)",
                content.len(),
                self.max_file_size
            ))
        } else {
            Ok(())
        }
    }

    /// Read a file's contents.
    pub fn read_file(&self, path: &Path) -> Result<String, String> {
        let canonical = self.validate(path, AccessType::Read)?;
        self.check_file_size(&canonical)?;

        std::fs::read_to_string(&canonical)
            .map_err(|e| format!("Failed to read file '{}': {}", path.display(), e))
    }

    /// Create a new file with the given content.
    ///
    /// Handles the case where parent directories don't exist yet by validating
    /// the closest existing ancestor against the sandbox, then creating the
    /// intermediate directories.
    pub fn create_file(&self, path: &Path, content: &str) -> Result<(), String> {
        self.check_content_size(content)?;

        // For file creation, we need to handle non-existent parent directories.
        // Validate by finding the closest existing ancestor and checking it's in the sandbox.
        let validated_path = self.validate_for_creation(path)?;

        // Create parent directories if they don't exist
        if let Some(parent) = validated_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create parent directories for '{}': {}",
                    path.display(),
                    e
                )
            })?;
        }

        std::fs::write(&validated_path, content)
            .map_err(|e| format!("Failed to create file '{}': {}", path.display(), e))?;

        info!(path = %validated_path.display(), "File created");
        Ok(())
    }

    /// Validate a path for file creation, handling non-existent parent directories.
    ///
    /// Walks up the path to find the closest existing ancestor, canonicalizes it,
    /// then appends the remaining components and checks against sandbox rules.
    fn validate_for_creation(&self, path: &Path) -> Result<PathBuf, String> {
        // First try normal validation (works if parent exists)
        if let Ok(canonical) = self.validate(path, AccessType::Write) {
            return Ok(canonical);
        }

        // Walk up to find the closest existing ancestor
        let mut current = path.to_path_buf();
        let mut remaining_components: Vec<std::ffi::OsString> = Vec::new();

        loop {
            if current.exists() {
                break;
            }
            let file_name = current.file_name().map(|f| f.to_os_string());
            let parent = current.parent().map(|p| p.to_path_buf());

            match (file_name, parent) {
                (Some(name), Some(par)) => {
                    remaining_components.push(name);
                    current = par;
                }
                _ => {
                    return Err(format!(
                        "Access denied: path '{}' is outside the allowed workspace directories",
                        path.display()
                    ));
                }
            }
        }

        // Validate the existing ancestor through the sandbox
        let canonical_ancestor = validate_path(&current, AccessType::Write, &self.filesystem_rules)
            .map_err(|_| {
                format!(
                    "Access denied: path '{}' is outside the allowed workspace directories",
                    path.display()
                )
            })?;

        // Reconstruct the full path from the canonical ancestor
        let mut full_path = canonical_ancestor;
        for component in remaining_components.into_iter().rev() {
            full_path = full_path.join(component);
        }

        Ok(full_path)
    }

    /// Overwrite a file with new content.
    pub fn write_file(&self, path: &Path, content: &str) -> Result<(), String> {
        self.check_content_size(content)?;
        let canonical = self.validate(path, AccessType::Write)?;
        self.check_file_size(&canonical)?;

        std::fs::write(&canonical, content)
            .map_err(|e| format!("Failed to write file '{}': {}", path.display(), e))?;

        info!(path = %canonical.display(), "File written");
        Ok(())
    }

    /// Edit a file using precise string replacement.
    ///
    /// Finds `old_str` in the file and replaces it with `new_str`.
    /// Rejects the operation if `old_str` is not found in the file.
    pub fn edit_file(&self, path: &Path, old_str: &str, new_str: &str) -> Result<String, String> {
        let canonical = self.validate(path, AccessType::Write)?;
        self.check_file_size(&canonical)?;

        let content = std::fs::read_to_string(&canonical)
            .map_err(|e| format!("Failed to read file '{}': {}", path.display(), e))?;

        if !content.contains(old_str) {
            return Err(format!(
                "String replacement failed: the match string was not found in '{}'",
                path.display()
            ));
        }

        let new_content = content.replacen(old_str, new_str, 1);

        // Check the resulting content size
        self.check_content_size(&new_content)?;

        std::fs::write(&canonical, &new_content)
            .map_err(|e| format!("Failed to write edited file '{}': {}", path.display(), e))?;

        info!(path = %canonical.display(), "File edited");
        Ok(new_content)
    }

    /// Delete a file.
    pub fn delete_file(&self, path: &Path) -> Result<(), String> {
        let canonical = self.validate(path, AccessType::Write)?;

        std::fs::remove_file(&canonical)
            .map_err(|e| format!("Failed to delete file '{}': {}", path.display(), e))?;

        info!(path = %canonical.display(), "File deleted");
        Ok(())
    }
}

// =============================================================================
// Tool Implementations
// =============================================================================

/// Tool for reading file contents.
pub struct FileReadTool {
    ops: Arc<FileOperations>,
}

impl FileReadTool {
    pub fn new(ops: Arc<FileOperations>) -> Self {
        Self { ops }
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file at the given path"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let path = Path::new(path_str);
        self.ops.read_file(path)
    }

    fn coding_only(&self) -> bool {
        true
    }
}

/// Tool for creating a new file with given content.
pub struct FileCreateTool {
    ops: Arc<FileOperations>,
}

impl FileCreateTool {
    pub fn new(ops: Arc<FileOperations>) -> Self {
        Self { ops }
    }
}

#[async_trait]
impl Tool for FileCreateTool {
    fn name(&self) -> &str {
        "file_create"
    }

    fn description(&self) -> &str {
        "Create a new file with the given content at the specified path"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to create"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the new file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        let path = Path::new(path_str);
        self.ops.create_file(path, content)?;
        Ok(format!("File created: {}", path_str))
    }

    fn coding_only(&self) -> bool {
        true
    }
}

/// Tool for overwriting a file with new content.
pub struct FileWriteTool {
    ops: Arc<FileOperations>,
}

impl FileWriteTool {
    pub fn new(ops: Arc<FileOperations>) -> Self {
        Self { ops }
    }
}

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Overwrite a file with new content at the specified path"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write"
                },
                "content": {
                    "type": "string",
                    "description": "The new content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        let path = Path::new(path_str);
        self.ops.write_file(path, content)?;
        Ok(format!("File written: {}", path_str))
    }

    fn coding_only(&self) -> bool {
        true
    }
}

/// Tool for precise string replacement in a file.
pub struct FileEditTool {
    ops: Arc<FileOperations>,
}

impl FileEditTool {
    pub fn new(ops: Arc<FileOperations>) -> Self {
        Self { ops }
    }
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing a specific string with a new string. \
         The old_str must exist exactly in the file."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to edit"
                },
                "old_str": {
                    "type": "string",
                    "description": "The exact string to find and replace"
                },
                "new_str": {
                    "type": "string",
                    "description": "The string to replace old_str with"
                }
            },
            "required": ["path", "old_str", "new_str"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let old_str = arguments
            .get("old_str")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: old_str".to_string())?;

        let new_str = arguments
            .get("new_str")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: new_str".to_string())?;

        let path = Path::new(path_str);
        self.ops.edit_file(path, old_str, new_str)?;
        Ok(format!("File edited: {}", path_str))
    }

    fn coding_only(&self) -> bool {
        true
    }
}

/// Tool for deleting a file.
pub struct FileDeleteTool {
    ops: Arc<FileOperations>,
}

impl FileDeleteTool {
    pub fn new(ops: Arc<FileOperations>) -> Self {
        Self { ops }
    }
}

#[async_trait]
impl Tool for FileDeleteTool {
    fn name(&self) -> &str {
        "file_delete"
    }

    fn description(&self) -> &str {
        "Delete a file at the specified path"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to delete"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let path = Path::new(path_str);
        self.ops.delete_file(path)?;
        Ok(format!("File deleted: {}", path_str))
    }

    fn coding_only(&self) -> bool {
        true
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

    /// Helper to create a FileOperations with a temp directory as the workspace.
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

    // --- FileOperations unit tests ---

    #[test]
    fn test_read_file_success() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("hello.txt");
        fs::write(&file_path, "Hello, world!").unwrap();

        let content = ops.read_file(&file_path).unwrap();
        assert_eq!(content, "Hello, world!");
    }

    #[test]
    fn test_read_file_not_found() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("nonexistent.txt");

        let result = ops.read_file(&file_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to read file"));
    }

    #[test]
    fn test_read_file_outside_sandbox() {
        let (tmp, _ops) = setup();
        // Create ops with a different allowed directory
        let allowed = tmp.path().join("allowed");
        fs::create_dir_all(&allowed).unwrap();
        let rules = vec![FilesystemRule {
            path: allowed.clone(),
            read: true,
            write: true,
            execute: false,
        }];
        let ops = FileOperations::new(rules);

        // Try to read a file outside the allowed directory
        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "secret").unwrap();

        let result = ops.read_file(&outside);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Access denied"));
    }

    #[test]
    fn test_read_file_exceeds_size_limit() {
        let (tmp, mut ops) = setup();
        ops.max_file_size = 10; // Set a tiny limit for testing

        let file_path = tmp.path().join("big.txt");
        fs::write(&file_path, "This content is longer than 10 bytes").unwrap();

        let result = ops.read_file(&file_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("File size limit exceeded"));
    }

    #[test]
    fn test_create_file_success() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("new_file.txt");

        ops.create_file(&file_path, "New content").unwrap();

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "New content");
    }

    #[test]
    fn test_create_file_with_subdirectory() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("sub").join("dir").join("file.txt");

        ops.create_file(&file_path, "Nested content").unwrap();

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Nested content");
    }

    #[test]
    fn test_create_file_outside_sandbox() {
        let (tmp, _) = setup();
        let allowed = tmp.path().join("allowed");
        fs::create_dir_all(&allowed).unwrap();
        let rules = vec![FilesystemRule {
            path: allowed.clone(),
            read: true,
            write: true,
            execute: false,
        }];
        let ops = FileOperations::new(rules);

        let outside = tmp.path().join("outside.txt");
        let result = ops.create_file(&outside, "content");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Access denied"));
    }

    #[test]
    fn test_create_file_exceeds_size_limit() {
        let (tmp, mut ops) = setup();
        ops.max_file_size = 5;

        let file_path = tmp.path().join("big.txt");
        let result = ops.create_file(&file_path, "This is too long");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Content size limit exceeded"));
    }

    #[test]
    fn test_write_file_success() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("existing.txt");
        fs::write(&file_path, "old content").unwrap();

        ops.write_file(&file_path, "new content").unwrap();

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "new content");
    }

    #[test]
    fn test_write_file_outside_sandbox() {
        let (tmp, _) = setup();
        let allowed = tmp.path().join("allowed");
        fs::create_dir_all(&allowed).unwrap();
        let rules = vec![FilesystemRule {
            path: allowed.clone(),
            read: true,
            write: true,
            execute: false,
        }];
        let ops = FileOperations::new(rules);

        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "old").unwrap();
        let result = ops.write_file(&outside, "new");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Access denied"));
    }

    #[test]
    fn test_edit_file_success() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("edit_me.txt");
        fs::write(&file_path, "Hello, world! Goodbye, world!").unwrap();

        let result = ops.edit_file(&file_path, "Hello", "Hi").unwrap();
        assert_eq!(result, "Hi, world! Goodbye, world!");

        let on_disk = fs::read_to_string(&file_path).unwrap();
        assert_eq!(on_disk, "Hi, world! Goodbye, world!");
    }

    #[test]
    fn test_edit_file_replaces_only_first_occurrence() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("multi.txt");
        fs::write(&file_path, "aaa bbb aaa").unwrap();

        let result = ops.edit_file(&file_path, "aaa", "ccc").unwrap();
        assert_eq!(result, "ccc bbb aaa");
    }

    #[test]
    fn test_edit_file_match_not_found() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("no_match.txt");
        fs::write(&file_path, "Hello, world!").unwrap();

        let result = ops.edit_file(&file_path, "nonexistent", "replacement");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("match string was not found"));
    }

    #[test]
    fn test_edit_file_outside_sandbox() {
        let (tmp, _) = setup();
        let allowed = tmp.path().join("allowed");
        fs::create_dir_all(&allowed).unwrap();
        let rules = vec![FilesystemRule {
            path: allowed.clone(),
            read: true,
            write: true,
            execute: false,
        }];
        let ops = FileOperations::new(rules);

        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "content").unwrap();
        let result = ops.edit_file(&outside, "content", "new");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Access denied"));
    }

    #[test]
    fn test_delete_file_success() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("delete_me.txt");
        fs::write(&file_path, "to be deleted").unwrap();

        ops.delete_file(&file_path).unwrap();
        assert!(!file_path.exists());
    }

    #[test]
    fn test_delete_file_not_found() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("nonexistent.txt");

        let result = ops.delete_file(&file_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to delete file"));
    }

    #[test]
    fn test_delete_file_outside_sandbox() {
        let (tmp, _) = setup();
        let allowed = tmp.path().join("allowed");
        fs::create_dir_all(&allowed).unwrap();
        let rules = vec![FilesystemRule {
            path: allowed.clone(),
            read: true,
            write: true,
            execute: false,
        }];
        let ops = FileOperations::new(rules);

        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "content").unwrap();
        let result = ops.delete_file(&outside);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Access denied"));
    }

    // --- Tool integration tests ---

    #[tokio::test]
    async fn test_file_read_tool() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("tool_read.txt");
        fs::write(&file_path, "tool content").unwrap();

        let tool = FileReadTool::new(Arc::new(ops));
        assert_eq!(tool.name(), "file_read");
        assert!(tool.coding_only());

        let result = tool
            .execute(json!({ "path": file_path.to_str().unwrap() }))
            .await;
        assert_eq!(result.unwrap(), "tool content");
    }

    #[tokio::test]
    async fn test_file_create_tool() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("tool_create.txt");

        let tool = FileCreateTool::new(Arc::new(ops));
        assert_eq!(tool.name(), "file_create");
        assert!(tool.coding_only());

        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "created by tool"
            }))
            .await;
        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "created by tool");
    }

    #[tokio::test]
    async fn test_file_write_tool() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("tool_write.txt");
        fs::write(&file_path, "old").unwrap();

        let tool = FileWriteTool::new(Arc::new(ops));
        assert_eq!(tool.name(), "file_write");
        assert!(tool.coding_only());

        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "new"
            }))
            .await;
        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "new");
    }

    #[tokio::test]
    async fn test_file_edit_tool() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("tool_edit.txt");
        fs::write(&file_path, "foo bar baz").unwrap();

        let tool = FileEditTool::new(Arc::new(ops));
        assert_eq!(tool.name(), "file_edit");
        assert!(tool.coding_only());

        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "old_str": "bar",
                "new_str": "qux"
            }))
            .await;
        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "foo qux baz");
    }

    #[tokio::test]
    async fn test_file_edit_tool_not_found() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("tool_edit_nf.txt");
        fs::write(&file_path, "foo bar baz").unwrap();

        let tool = FileEditTool::new(Arc::new(ops));

        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "old_str": "nonexistent",
                "new_str": "replacement"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("match string was not found"));
    }

    #[tokio::test]
    async fn test_file_delete_tool() {
        let (tmp, ops) = setup();
        let file_path = tmp.path().join("tool_delete.txt");
        fs::write(&file_path, "delete me").unwrap();

        let tool = FileDeleteTool::new(Arc::new(ops));
        assert_eq!(tool.name(), "file_delete");
        assert!(tool.coding_only());

        let result = tool
            .execute(json!({ "path": file_path.to_str().unwrap() }))
            .await;
        assert!(result.is_ok());
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_tool_missing_parameters() {
        let (_, ops) = setup();
        let ops = Arc::new(ops);

        let read_tool = FileReadTool::new(Arc::clone(&ops));
        let result = read_tool.execute(json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required parameter"));

        let create_tool = FileCreateTool::new(Arc::clone(&ops));
        let result = create_tool.execute(json!({"path": "/tmp/x"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required parameter"));

        let edit_tool = FileEditTool::new(Arc::clone(&ops));
        let result = edit_tool
            .execute(json!({"path": "/tmp/x", "old_str": "a"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required parameter"));
    }

    #[test]
    fn test_path_traversal_blocked() {
        let (tmp, ops) = setup();
        // Create a file outside via traversal
        let outside = tmp.path().join("..").join("escape.txt");
        let result = ops.read_file(&outside);
        assert!(result.is_err());
    }
}
