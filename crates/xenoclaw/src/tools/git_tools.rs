//! Git Tools — Tool trait implementations wrapping `coding_module::GitOperations`.
//!
//! Provides:
//! - `git_status` — show working tree status (changed files)
//! - `git_diff` — show unified diff of working tree changes
//! - `git_commit` — stage all changes and create a commit

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

use agent_core::tool_registry::Tool;
use coding_module::GitOperations;

// =============================================================================
// GitStatusTool
// =============================================================================

/// Tool that shows the current git working tree status (changed files).
pub struct GitStatusTool {
    git_ops: Arc<GitOperations>,
}

impl GitStatusTool {
    /// Create a new GitStatusTool wrapping the given GitOperations.
    pub fn new(git_ops: Arc<GitOperations>) -> Self {
        Self { git_ops }
    }
}

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show the current git working tree status, listing modified, added, and deleted files."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "repo_dir": {
                    "type": "string",
                    "description": "Path to the git repository directory"
                }
            },
            "required": ["repo_dir"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let repo_dir = arguments
            .get("repo_dir")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'repo_dir'".to_string())?;

        debug!(repo_dir = %repo_dir, "Executing git_status tool");

        let diff_result = self
            .git_ops
            .diff(Path::new(repo_dir))
            .await
            .map_err(|e| e.to_string())?;

        let response = if diff_result.changed_files.is_empty() {
            json!({
                "status": "clean",
                "changed_files": []
            })
        } else {
            json!({
                "status": "dirty",
                "changed_files": diff_result.changed_files
            })
        };

        Ok(response.to_string())
    }

    fn coding_only(&self) -> bool {
        true
    }
}

// =============================================================================
// GitDiffTool
// =============================================================================

/// Tool that shows the unified diff of working tree changes.
pub struct GitDiffTool {
    git_ops: Arc<GitOperations>,
}

impl GitDiffTool {
    /// Create a new GitDiffTool wrapping the given GitOperations.
    pub fn new(git_ops: Arc<GitOperations>) -> Self {
        Self { git_ops }
    }
}

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show the unified diff of all staged and unstaged changes in the repository."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "repo_dir": {
                    "type": "string",
                    "description": "Path to the git repository directory"
                }
            },
            "required": ["repo_dir"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let repo_dir = arguments
            .get("repo_dir")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'repo_dir'".to_string())?;

        debug!(repo_dir = %repo_dir, "Executing git_diff tool");

        let diff_result = self
            .git_ops
            .diff(Path::new(repo_dir))
            .await
            .map_err(|e| e.to_string())?;

        let response = json!({
            "diff_text": diff_result.diff_text,
            "changed_files": diff_result.changed_files
        });

        Ok(response.to_string())
    }

    fn coding_only(&self) -> bool {
        true
    }
}

// =============================================================================
// GitCommitTool
// =============================================================================

/// Tool that stages all changes and creates a commit with the given message.
pub struct GitCommitTool {
    git_ops: Arc<GitOperations>,
}

impl GitCommitTool {
    /// Create a new GitCommitTool wrapping the given GitOperations.
    pub fn new(git_ops: Arc<GitOperations>) -> Self {
        Self { git_ops }
    }
}

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Stage all changes and create a git commit with the given message. \
         The commit message should follow conventional commit format: \
         type(scope): description"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "repo_dir": {
                    "type": "string",
                    "description": "Path to the git repository directory"
                },
                "message": {
                    "type": "string",
                    "description": "The commit message (should be ≤72 chars for the summary line)"
                }
            },
            "required": ["repo_dir", "message"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let repo_dir = arguments
            .get("repo_dir")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'repo_dir'".to_string())?;

        let message = arguments
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'message'".to_string())?;

        debug!(repo_dir = %repo_dir, message = %message, "Executing git_commit tool");

        let commit_result = self
            .git_ops
            .commit(Path::new(repo_dir), message)
            .await
            .map_err(|e| e.to_string())?;

        let response = json!({
            "commit_hash": commit_result.commit_hash
        });

        Ok(response.to_string())
    }

    fn coding_only(&self) -> bool {
        true
    }
}
