//! Git Operations — clone, pull, push, commit, branch, merge, and diff.
//!
//! Provides a `GitOperations` struct that shells out to `git` via
//! `tokio::process::Command` for all operations.
//!
//! Key behaviors:
//! - Restrict operations to configured repository directories
//! - Support SSH key and personal access token authentication
//! - Report push conflicts (branch name, affected files) instead of force-pushing
//! - Report auth failures without retrying
//! - Generate commit messages: summary ≤72 chars with type and component

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::process::Command;
use tracing::info;

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur during git operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum GitError {
    /// The repository directory is not in the configured list.
    #[error("Directory '{path}' is not an authorized repository path")]
    UnauthorizedDirectory { path: String },

    /// A push failed due to remote conflicts.
    #[error("Push conflict on branch '{branch}': affected files: {affected_files:?}")]
    PushConflict {
        branch: String,
        affected_files: Vec<String>,
    },

    /// Authentication with the remote failed.
    #[error("Authentication failed for remote '{remote}'")]
    AuthFailed { remote: String },

    /// The git command failed with a non-zero exit code.
    #[error("Git command failed: {message}")]
    CommandFailed { message: String },

    /// Failed to spawn the git process.
    #[error("Failed to spawn git: {reason}")]
    SpawnFailed { reason: String },

    /// The directory is not a git repository.
    #[error("Not a git repository: '{path}'")]
    NotARepository { path: String },
}

// =============================================================================
// Authentication
// =============================================================================

/// Authentication method for git remotes.
#[derive(Debug, Clone)]
pub enum GitAuth {
    /// SSH key authentication.
    SshKey {
        /// Path to the private key file.
        private_key_path: PathBuf,
    },
    /// Personal access token authentication.
    PersonalAccessToken {
        /// The token value.
        token: String,
    },
    /// No authentication (for public repos or pre-configured SSH agent).
    None,
}

// =============================================================================
// Git Operation Results
// =============================================================================

/// Result of a git clone operation.
#[derive(Debug, Clone)]
pub struct CloneResult {
    /// The directory the repo was cloned into.
    pub target_dir: PathBuf,
}

/// Result of a git commit operation.
#[derive(Debug, Clone)]
pub struct CommitResult {
    /// The commit hash.
    pub commit_hash: String,
}

/// Result of a git diff operation.
#[derive(Debug, Clone)]
pub struct DiffResult {
    /// The unified diff output.
    pub diff_text: String,
    /// Files that have changes.
    pub changed_files: Vec<String>,
}

/// Result of a git merge operation.
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// Whether the merge was successful.
    pub success: bool,
    /// Merge output message.
    pub message: String,
}

// =============================================================================
// GitOperations Configuration
// =============================================================================

/// Configuration for git operations.
#[derive(Debug, Clone)]
pub struct GitOperationsConfig {
    /// Directories that are authorized for git operations.
    pub repository_dirs: Vec<PathBuf>,
}

impl GitOperationsConfig {
    /// Create a config from a CodingConfig.
    pub fn from_coding_config(config: &common::config::CodingConfig) -> Self {
        Self {
            repository_dirs: config.repository_dirs.clone(),
        }
    }
}

// =============================================================================
// GitOperations
// =============================================================================

/// Executes git operations with directory restriction and authentication support.
#[derive(Debug, Clone)]
pub struct GitOperations {
    config: Arc<GitOperationsConfig>,
}

impl GitOperations {
    /// Create a new GitOperations with the given configuration.
    pub fn new(config: GitOperationsConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// Validate that the given directory is within the configured repository dirs.
    fn validate_repo_dir(&self, repo_dir: &Path) -> Result<(), GitError> {
        let canonical = std::fs::canonicalize(repo_dir).unwrap_or_else(|_| repo_dir.to_path_buf());

        for allowed_dir in &self.config.repository_dirs {
            let allowed_canonical =
                std::fs::canonicalize(allowed_dir).unwrap_or_else(|_| allowed_dir.clone());
            if canonical.starts_with(&allowed_canonical) {
                return Ok(());
            }
        }

        Err(GitError::UnauthorizedDirectory {
            path: repo_dir.display().to_string(),
        })
    }

    /// Build environment variables for git authentication.
    fn auth_env(&self, auth: &GitAuth) -> Vec<(String, String)> {
        match auth {
            GitAuth::SshKey { private_key_path } => {
                let ssh_command = format!(
                    "ssh -i {} -o StrictHostKeyChecking=no -o BatchMode=yes",
                    private_key_path.display()
                );
                vec![("GIT_SSH_COMMAND".to_string(), ssh_command)]
            }
            GitAuth::PersonalAccessToken { .. } => {
                // Token is embedded in the URL during clone, or via credential helper.
                // We set GIT_TERMINAL_PROMPT=0 to prevent interactive prompts.
                vec![("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())]
            }
            GitAuth::None => {
                vec![("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())]
            }
        }
    }

    /// Inject a personal access token into a git URL.
    fn inject_token_in_url(url: &str, token: &str) -> String {
        if url.starts_with("https://") {
            // Transform https://host/path to https://token@host/path
            let without_scheme = &url["https://".len()..];
            format!("https://{}@{}", token, without_scheme)
        } else if url.starts_with("http://") {
            let without_scheme = &url["http://".len()..];
            format!("http://{}@{}", token, without_scheme)
        } else {
            // SSH or other URL — return as-is
            url.to_string()
        }
    }

    /// Run a git command and return its output.
    async fn run_git(
        &self,
        args: &[&str],
        cwd: &Path,
        auth: &GitAuth,
    ) -> Result<String, GitError> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(cwd).kill_on_drop(true);

        // Set auth environment variables
        for (key, value) in self.auth_env(auth) {
            cmd.env(&key, &value);
        }

        let output = cmd.output().await.map_err(|e| GitError::SpawnFailed {
            reason: e.to_string(),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(stdout)
        } else {
            // Check for auth failure patterns
            if Self::is_auth_failure(&stderr) {
                return Err(GitError::AuthFailed {
                    remote: Self::extract_remote(&stderr),
                });
            }
            Err(GitError::CommandFailed {
                message: if stderr.is_empty() { stdout } else { stderr },
            })
        }
    }

    /// Check if stderr indicates an authentication failure.
    fn is_auth_failure(stderr: &str) -> bool {
        let lower = stderr.to_lowercase();
        lower.contains("authentication failed")
            || lower.contains("could not read from remote")
            || lower.contains("permission denied")
            || lower.contains("invalid credentials")
            || lower.contains("401")
            || lower.contains("403")
    }

    /// Extract the remote name/URL from an error message.
    fn extract_remote(stderr: &str) -> String {
        // Try to find a URL or remote name in the error
        for line in stderr.lines() {
            if line.contains("fatal:") || line.contains("remote:") {
                // Try to extract URL-like patterns
                for word in line.split_whitespace() {
                    if word.contains("://") || word.contains("@") {
                        return word.trim_matches('\'').to_string();
                    }
                }
            }
        }
        "unknown".to_string()
    }

    /// Clone a repository.
    pub async fn clone(
        &self,
        url: &str,
        target_dir: &Path,
        auth: &GitAuth,
    ) -> Result<CloneResult, GitError> {
        // Validate target directory is within allowed repository dirs
        // For clone, we check the parent directory
        let parent = target_dir.parent().unwrap_or(target_dir);
        self.validate_repo_dir(parent)?;

        let effective_url = match auth {
            GitAuth::PersonalAccessToken { token } => Self::inject_token_in_url(url, token),
            _ => url.to_string(),
        };

        info!(url = %url, target = %target_dir.display(), "Cloning repository");

        let target_str = target_dir.to_string_lossy().to_string();
        let mut cmd = Command::new("git");
        cmd.args(["clone", &effective_url, &target_str])
            .kill_on_drop(true);

        for (key, value) in self.auth_env(auth) {
            cmd.env(&key, &value);
        }

        let output = cmd.output().await.map_err(|e| GitError::SpawnFailed {
            reason: e.to_string(),
        })?;

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            if Self::is_auth_failure(&stderr) {
                return Err(GitError::AuthFailed {
                    remote: url.to_string(),
                });
            }
            return Err(GitError::CommandFailed { message: stderr });
        }

        Ok(CloneResult {
            target_dir: target_dir.to_path_buf(),
        })
    }

    /// Pull latest changes from the remote.
    pub async fn pull(&self, repo_dir: &Path, auth: &GitAuth) -> Result<String, GitError> {
        self.validate_repo_dir(repo_dir)?;

        info!(repo = %repo_dir.display(), "Pulling latest changes");
        self.run_git(&["pull"], repo_dir, auth).await
    }

    /// Push commits to the remote.
    ///
    /// On conflict, reports the branch name and affected files instead of
    /// force-pushing.
    pub async fn push(&self, repo_dir: &Path, auth: &GitAuth) -> Result<String, GitError> {
        self.validate_repo_dir(repo_dir)?;

        info!(repo = %repo_dir.display(), "Pushing to remote");

        // Get current branch name
        let branch = self
            .run_git(&["rev-parse", "--abbrev-ref", "HEAD"], repo_dir, &GitAuth::None)
            .await
            .unwrap_or_else(|_| "unknown".to_string())
            .trim()
            .to_string();

        // Attempt push
        let result = self.run_git(&["push"], repo_dir, auth).await;

        match result {
            Ok(output) => Ok(output),
            Err(GitError::CommandFailed { message }) => {
                // Check if this is a conflict/rejection
                if Self::is_push_conflict(&message) {
                    let affected_files = self.get_conflicting_files(repo_dir).await;
                    Err(GitError::PushConflict {
                        branch,
                        affected_files,
                    })
                } else if Self::is_auth_failure(&message) {
                    Err(GitError::AuthFailed {
                        remote: Self::extract_remote(&message),
                    })
                } else {
                    Err(GitError::CommandFailed { message })
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Check if a push error indicates a conflict/rejection.
    fn is_push_conflict(stderr: &str) -> bool {
        let lower = stderr.to_lowercase();
        lower.contains("rejected")
            || lower.contains("non-fast-forward")
            || lower.contains("failed to push")
            || lower.contains("fetch first")
    }

    /// Get the list of files that would conflict on push.
    async fn get_conflicting_files(&self, repo_dir: &Path) -> Vec<String> {
        // Fetch and compare to find divergent files
        let _ = self
            .run_git(&["fetch"], repo_dir, &GitAuth::None)
            .await;

        let diff_output = self
            .run_git(
                &["diff", "--name-only", "HEAD..@{upstream}"],
                repo_dir,
                &GitAuth::None,
            )
            .await
            .unwrap_or_default();

        diff_output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect()
    }

    /// Create a commit with the given message.
    pub async fn commit(
        &self,
        repo_dir: &Path,
        message: &str,
    ) -> Result<CommitResult, GitError> {
        self.validate_repo_dir(repo_dir)?;

        info!(repo = %repo_dir.display(), "Creating commit");

        // Stage all changes
        self.run_git(&["add", "-A"], repo_dir, &GitAuth::None).await?;

        // Create the commit
        self.run_git(&["commit", "-m", message], repo_dir, &GitAuth::None)
            .await?;

        // Get the commit hash
        let hash = self
            .run_git(&["rev-parse", "HEAD"], repo_dir, &GitAuth::None)
            .await?
            .trim()
            .to_string();

        Ok(CommitResult { commit_hash: hash })
    }

    /// Create a branch and optionally check it out.
    pub async fn branch(
        &self,
        repo_dir: &Path,
        name: &str,
        checkout: bool,
    ) -> Result<String, GitError> {
        self.validate_repo_dir(repo_dir)?;

        info!(repo = %repo_dir.display(), branch = %name, checkout = checkout, "Branch operation");

        if checkout {
            self.run_git(&["checkout", "-b", name], repo_dir, &GitAuth::None)
                .await
        } else {
            self.run_git(&["branch", name], repo_dir, &GitAuth::None)
                .await
        }
    }

    /// Merge a branch into the current branch.
    pub async fn merge(
        &self,
        repo_dir: &Path,
        branch: &str,
    ) -> Result<MergeResult, GitError> {
        self.validate_repo_dir(repo_dir)?;

        info!(repo = %repo_dir.display(), branch = %branch, "Merging branch");

        match self
            .run_git(&["merge", branch], repo_dir, &GitAuth::None)
            .await
        {
            Ok(output) => Ok(MergeResult {
                success: true,
                message: output,
            }),
            Err(GitError::CommandFailed { message }) => Ok(MergeResult {
                success: false,
                message,
            }),
            Err(e) => Err(e),
        }
    }

    /// Get the current diff (staged and unstaged changes).
    pub async fn diff(&self, repo_dir: &Path) -> Result<DiffResult, GitError> {
        self.validate_repo_dir(repo_dir)?;

        // Get unstaged diff
        let unstaged = self
            .run_git(&["diff"], repo_dir, &GitAuth::None)
            .await
            .unwrap_or_default();

        // Get staged diff
        let staged = self
            .run_git(&["diff", "--cached"], repo_dir, &GitAuth::None)
            .await
            .unwrap_or_default();

        // Combine diffs
        let diff_text = if !staged.is_empty() && !unstaged.is_empty() {
            format!("{}\n{}", staged, unstaged)
        } else if !staged.is_empty() {
            staged
        } else {
            unstaged
        };

        // Get list of changed files
        let status_output = self
            .run_git(&["status", "--porcelain"], repo_dir, &GitAuth::None)
            .await
            .unwrap_or_default();

        let changed_files: Vec<String> = status_output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                // git status --porcelain format: "XY filename" (2 status chars + space + filename)
                if l.len() > 3 {
                    l[3..].to_string()
                } else {
                    l.trim().to_string()
                }
            })
            .collect();

        Ok(DiffResult {
            diff_text,
            changed_files,
        })
    }

    /// Generate a commit message with a summary ≤72 chars and body listing
    /// modified files.
    ///
    /// Format:
    /// ```text
    /// <change_type>(<component>): <description>
    ///
    /// Modified files:
    /// - path/to/file1
    /// - path/to/file2
    /// ```
    pub async fn generate_commit_message(
        &self,
        repo_dir: &Path,
        change_type: &str,
        component: &str,
    ) -> Result<String, GitError> {
        self.validate_repo_dir(repo_dir)?;

        // Get the list of modified files
        let status_output = self
            .run_git(&["status", "--porcelain"], repo_dir, &GitAuth::None)
            .await
            .unwrap_or_default();

        let modified_files: Vec<String> = status_output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                // git status --porcelain format: "XY filename"
                if l.len() > 3 {
                    l[3..].to_string()
                } else {
                    l.trim().to_string()
                }
            })
            .collect();

        // Build the summary line, ensuring ≤72 chars
        let summary = Self::build_summary(change_type, component, &modified_files);

        // Build the body
        let body = if modified_files.is_empty() {
            String::new()
        } else {
            let file_list: String = modified_files
                .iter()
                .map(|f| format!("- {}", f))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\nModified files:\n{}", file_list)
        };

        Ok(format!("{}{}", summary, body))
    }

    /// Build a summary line ≤72 characters.
    ///
    /// This is exposed for property-based testing of commit message format.
    pub fn build_summary(change_type: &str, component: &str, files: &[String]) -> String {
        // Format: "type(component): description"
        let prefix = format!("{}({}): ", change_type, component);
        let max_desc_len = 72usize.saturating_sub(prefix.len());

        let description = if files.is_empty() {
            "no changes".to_string()
        } else if files.len() == 1 {
            let desc = format!("update {}", files[0]);
            if desc.len() <= max_desc_len {
                desc
            } else {
                desc[..max_desc_len].to_string()
            }
        } else {
            let desc = format!("update {} files", files.len());
            if desc.len() <= max_desc_len {
                desc
            } else {
                desc[..max_desc_len].to_string()
            }
        };

        let summary = format!("{}{}", prefix, description);
        // Final safety truncation to 72 chars
        if summary.len() > 72 {
            summary[..72].to_string()
        } else {
            summary
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(dir: &Path) -> GitOperationsConfig {
        GitOperationsConfig {
            repository_dirs: vec![dir.to_path_buf()],
        }
    }

    #[test]
    fn test_validate_repo_dir_allowed() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));
        assert!(ops.validate_repo_dir(tmp.path()).is_ok());
    }

    #[test]
    fn test_validate_repo_dir_subdirectory_allowed() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("subdir");
        std::fs::create_dir_all(&sub).unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));
        assert!(ops.validate_repo_dir(&sub).is_ok());
    }

    #[test]
    fn test_validate_repo_dir_unauthorized() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));
        let result = ops.validate_repo_dir(Path::new("/etc"));
        assert!(matches!(result, Err(GitError::UnauthorizedDirectory { .. })));
    }

    #[test]
    fn test_validate_repo_dir_empty_config() {
        let ops = GitOperations::new(GitOperationsConfig {
            repository_dirs: vec![],
        });
        let result = ops.validate_repo_dir(Path::new("/tmp"));
        assert!(matches!(result, Err(GitError::UnauthorizedDirectory { .. })));
    }

    #[test]
    fn test_inject_token_in_https_url() {
        let url = "https://github.com/user/repo.git";
        let result = GitOperations::inject_token_in_url(url, "mytoken123");
        assert_eq!(result, "https://mytoken123@github.com/user/repo.git");
    }

    #[test]
    fn test_inject_token_in_ssh_url_unchanged() {
        let url = "git@github.com:user/repo.git";
        let result = GitOperations::inject_token_in_url(url, "mytoken123");
        assert_eq!(result, url);
    }

    #[test]
    fn test_is_auth_failure_patterns() {
        assert!(GitOperations::is_auth_failure("Authentication failed for 'https://github.com'"));
        assert!(GitOperations::is_auth_failure("Permission denied (publickey)"));
        assert!(GitOperations::is_auth_failure("fatal: could not read from remote repository"));
        assert!(!GitOperations::is_auth_failure("Everything up-to-date"));
    }

    #[test]
    fn test_is_push_conflict_patterns() {
        assert!(GitOperations::is_push_conflict(
            "! [rejected] main -> main (non-fast-forward)"
        ));
        assert!(GitOperations::is_push_conflict(
            "error: failed to push some refs"
        ));
        assert!(GitOperations::is_push_conflict(
            "hint: Updates were rejected because the tip of your current branch is behind"
        ));
        assert!(!GitOperations::is_push_conflict("Everything up-to-date"));
    }

    #[test]
    fn test_build_summary_single_file() {
        let files = vec!["src/main.rs".to_string()];
        let summary = GitOperations::build_summary("feat", "core", &files);
        assert!(summary.len() <= 72);
        assert!(summary.starts_with("feat(core): "));
        assert!(summary.contains("src/main.rs"));
    }

    #[test]
    fn test_build_summary_multiple_files() {
        let files = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "Cargo.toml".to_string(),
        ];
        let summary = GitOperations::build_summary("fix", "module", &files);
        assert!(summary.len() <= 72);
        assert!(summary.starts_with("fix(module): "));
        assert!(summary.contains("3 files"));
    }

    #[test]
    fn test_build_summary_no_files() {
        let files: Vec<String> = vec![];
        let summary = GitOperations::build_summary("chore", "deps", &files);
        assert!(summary.len() <= 72);
        assert!(summary.contains("no changes"));
    }

    #[test]
    fn test_build_summary_truncation() {
        let files = vec!["a_very_long_filename_that_would_exceed_the_limit_if_not_truncated_properly.rs".to_string()];
        let summary = GitOperations::build_summary("refactor", "long-component-name", &files);
        assert!(summary.len() <= 72, "Summary was {} chars: {}", summary.len(), summary);
    }

    #[test]
    fn test_auth_env_ssh_key() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));
        let key_path = PathBuf::from("/home/user/.ssh/id_rsa");
        let auth = GitAuth::SshKey {
            private_key_path: key_path.clone(),
        };
        let env = ops.auth_env(&auth);
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "GIT_SSH_COMMAND");
        assert!(env[0].1.contains(&key_path.display().to_string()));
    }

    #[test]
    fn test_auth_env_pat() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));
        let auth = GitAuth::PersonalAccessToken {
            token: "ghp_test123".to_string(),
        };
        let env = ops.auth_env(&auth);
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "GIT_TERMINAL_PROMPT");
        assert_eq!(env[0].1, "0");
    }

    #[test]
    fn test_auth_env_none() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));
        let auth = GitAuth::None;
        let env = ops.auth_env(&auth);
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "GIT_TERMINAL_PROMPT");
    }

    // Integration tests that require git to be installed

    #[tokio::test]
    async fn test_commit_in_repo() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));

        // Initialize a git repo
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Configure git user for the test repo
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Create a file
        std::fs::write(tmp.path().join("test.txt"), "hello").unwrap();

        // Commit
        let result = ops.commit(tmp.path(), "feat(test): initial commit").await;
        assert!(result.is_ok());
        let commit = result.unwrap();
        assert!(!commit.commit_hash.is_empty());
    }

    #[tokio::test]
    async fn test_diff_in_repo() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));

        // Initialize a git repo
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Create and commit a file
        std::fs::write(tmp.path().join("file.txt"), "original").unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Modify the file
        std::fs::write(tmp.path().join("file.txt"), "modified").unwrap();

        // Get diff
        let result = ops.diff(tmp.path()).await;
        assert!(result.is_ok());
        let diff = result.unwrap();
        assert!(!diff.diff_text.is_empty());
        assert!(diff.changed_files.contains(&"file.txt".to_string()));
    }

    #[tokio::test]
    async fn test_branch_in_repo() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));

        // Initialize a git repo with an initial commit
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        std::fs::write(tmp.path().join("init.txt"), "init").unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Create a branch without checkout
        let result = ops.branch(tmp.path(), "feature-branch", false).await;
        assert!(result.is_ok());

        // Create and checkout a branch
        let result = ops.branch(tmp.path(), "dev-branch", true).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_merge_in_repo() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));

        // Initialize repo
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Initial commit on main
        std::fs::write(tmp.path().join("main.txt"), "main content").unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial on main"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Create feature branch and add a commit
        Command::new("git")
            .args(["checkout", "-b", "feature"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        std::fs::write(tmp.path().join("feature.txt"), "feature content").unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "feature commit"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Switch back to main and merge
        Command::new("git")
            .args(["checkout", "master"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        let result = ops.merge(tmp.path(), "feature").await;
        assert!(result.is_ok());
        let merge_result = result.unwrap();
        assert!(merge_result.success);
    }

    #[tokio::test]
    async fn test_generate_commit_message() {
        let tmp = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));

        // Initialize repo
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .await
            .unwrap();

        // Create some files
        std::fs::write(tmp.path().join("src.rs"), "fn main() {}").unwrap();
        std::fs::write(tmp.path().join("lib.rs"), "pub mod foo;").unwrap();

        let result = ops
            .generate_commit_message(tmp.path(), "feat", "core")
            .await;
        assert!(result.is_ok());
        let msg = result.unwrap();

        // Check summary line is ≤72 chars
        let summary = msg.lines().next().unwrap();
        assert!(
            summary.len() <= 72,
            "Summary too long ({} chars): {}",
            summary.len(),
            summary
        );
        assert!(summary.starts_with("feat(core): "));

        // Check body lists files
        assert!(msg.contains("Modified files:"));
    }

    #[tokio::test]
    async fn test_operations_on_unauthorized_dir() {
        let tmp = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let ops = GitOperations::new(test_config(tmp.path()));

        // All operations should fail on unauthorized directory
        let result = ops.diff(other.path()).await;
        assert!(matches!(result, Err(GitError::UnauthorizedDirectory { .. })));

        let result = ops.commit(other.path(), "test").await;
        assert!(matches!(result, Err(GitError::UnauthorizedDirectory { .. })));

        let result = ops.branch(other.path(), "test", false).await;
        assert!(matches!(result, Err(GitError::UnauthorizedDirectory { .. })));

        let result = ops.merge(other.path(), "test").await;
        assert!(matches!(result, Err(GitError::UnauthorizedDirectory { .. })));

        let result = ops.pull(other.path(), &GitAuth::None).await;
        assert!(matches!(result, Err(GitError::UnauthorizedDirectory { .. })));

        let result = ops.push(other.path(), &GitAuth::None).await;
        assert!(matches!(result, Err(GitError::UnauthorizedDirectory { .. })));
    }
}
