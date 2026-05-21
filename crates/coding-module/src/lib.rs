//! Coding Module — optional module providing software development capabilities.
//!
//! Responsibilities:
//! - File read/create/edit/delete with undo history
//! - Shell command execution with streaming output
//! - Git operations with authentication
//! - LSP client management
//! - Diff generation and rendering

pub mod diff_renderer;
pub mod diff_tracker;
pub mod file_ops;
pub mod git_ops;
pub mod lsp_client;
pub mod shell_executor;
pub mod shell_tool;
pub mod undo_history;

pub use diff_renderer::{DiffRenderer, RenderError};
pub use diff_tracker::{
    generate_unified_diff, is_binary_content, ChangeTracker, DiffHunk, DiffLine, FileChange,
    SessionChangeSummary, UnifiedDiff,
};
pub use git_ops::{
    CloneResult, CommitResult, DiffResult as GitDiffResult, GitAuth, GitError, GitOperations,
    GitOperationsConfig, MergeResult,
};
pub use lsp_client::{
    Diagnostic, DiagnosticSeverity, Location, LspClientManager, LspError, TextEdit, TextRange,
};
pub use shell_executor::{
    CommandOutput, OutputChunk, ProcessInfo, ShellError, ShellExecutor, ShellExecutorConfig,
};
pub use shell_tool::ShellCommandTool;
pub use undo_history::{TrackedFileOperations, UndoEntry, UndoHistory, UndoResult};
