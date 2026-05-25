//! Built-in tool registration for the XenoClaw agent runtime.
//!
//! This module provides `register_builtin_tools()` which constructs and registers
//! all built-in tools into a `ToolRegistry`. Tools that cannot be initialized
//! (e.g., memory tools without a database pool) are skipped with a warning.

pub mod git_tools;
pub mod image_tools;
pub mod memory_tools;
pub mod task_tools;

use std::path::PathBuf;
use std::sync::Arc;

use sqlx::sqlite::SqlitePool;
use tracing::{info, warn};

use agent_core::ToolRegistry;
use coding_module::file_ops::{
    FileCreateTool, FileDeleteTool, FileEditTool, FileOperations, FileReadTool, FileWriteTool,
};
use coding_module::git_ops::{GitOperations, GitOperationsConfig};
use coding_module::shell_executor::{ShellExecutor, ShellExecutorConfig};
use coding_module::ShellCommandTool;
use common::config::{CodingConfig, FilesystemRule};
use task_scheduler::Scheduler;

use self::git_tools::{GitCommitTool, GitDiffTool, GitStatusTool};
use self::image_tools::ViewImageTool;
use self::memory_tools::{MemorySearchTool, MemoryStoreTool};
use self::task_tools::{TaskCreateTool, TaskListTool};

/// Configuration for built-in tool registration.
pub struct BuiltinToolsConfig {
    /// Coding module configuration (None = coding tools disabled).
    pub coding: Option<CodingConfig>,
    /// Filesystem sandbox rules from the security config.
    pub filesystem_rules: Vec<FilesystemRule>,
    /// SQLite connection pool for memory tools (None = memory tools disabled).
    pub db_pool: Option<SqlitePool>,
    /// Task scheduler instance for task tools.
    pub scheduler: Arc<Scheduler>,
    /// Workspace root — used by the view_image tool to resolve upload paths.
    pub workspace_dir: PathBuf,
    /// Whether the active model supports image input (vision). Drives the
    /// view_image tool's fail-safe.
    pub vision_supported: bool,
}

/// Register all built-in tools into a new `ToolRegistry`.
///
/// Tools that cannot be initialized due to missing configuration or resources
/// are skipped with a warning log. The function always returns a valid registry,
/// even if some tools could not be registered.
pub fn register_builtin_tools(config: BuiltinToolsConfig) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    let mut registered_count = 0u32;

    // -------------------------------------------------------------------------
    // File tools + Shell tool (require coding config)
    // -------------------------------------------------------------------------
    if let Some(ref coding_config) = config.coding {
        // File operations tools
        let file_ops = Arc::new(FileOperations::new(config.filesystem_rules.clone()));

        registry.register(Arc::new(FileReadTool::new(Arc::clone(&file_ops))));
        registry.register(Arc::new(FileCreateTool::new(Arc::clone(&file_ops))));
        registry.register(Arc::new(FileWriteTool::new(Arc::clone(&file_ops))));
        registry.register(Arc::new(FileEditTool::new(Arc::clone(&file_ops))));
        registry.register(Arc::new(FileDeleteTool::new(Arc::clone(&file_ops))));
        registered_count += 5;

        // Shell command tool
        let shell_config = ShellExecutorConfig {
            command_allowlist: coding_config.command_allowlist.clone(),
            command_blocklist: coding_config.command_blocklist.clone(),
            default_timeout_seconds: coding_config.shell_timeout_seconds,
            max_concurrent_shells: coding_config.max_concurrent_shells as usize,
        };
        let shell_executor = ShellExecutor::new(shell_config);
        registry.register(Arc::new(ShellCommandTool::new(shell_executor)));
        registered_count += 1;

        // Git tools (require repository_dirs in coding config)
        if !coding_config.repository_dirs.is_empty() {
            let git_config = GitOperationsConfig::from_coding_config(coding_config);
            let git_ops = Arc::new(GitOperations::new(git_config));

            registry.register(Arc::new(GitStatusTool::new(Arc::clone(&git_ops))));
            registry.register(Arc::new(GitDiffTool::new(Arc::clone(&git_ops))));
            registry.register(Arc::new(GitCommitTool::new(Arc::clone(&git_ops))));
            registered_count += 3;
        } else {
            warn!("Git tools disabled: no repository_dirs configured in [coding] section");
        }
    } else {
        warn!("Coding tools disabled: no [coding] section in configuration");
    }

    // -------------------------------------------------------------------------
    // Memory tools (require database pool)
    // -------------------------------------------------------------------------
    if let Some(pool) = config.db_pool {
        let store = Arc::new(memory_store::KnowledgeStore::with_defaults(pool));

        registry.register(Arc::new(MemorySearchTool::new(Arc::clone(&store))));
        registry.register(Arc::new(MemoryStoreTool::new(Arc::clone(&store))));
        registered_count += 2;
    } else {
        warn!("Memory tools disabled: no database pool available");
    }

    // -------------------------------------------------------------------------
    // Task tools (always available — scheduler is required)
    // -------------------------------------------------------------------------
    registry.register(Arc::new(TaskCreateTool::new(Arc::clone(&config.scheduler))));
    registry.register(Arc::new(TaskListTool::new(Arc::clone(&config.scheduler))));
    registered_count += 2;

    // -------------------------------------------------------------------------
    // Image viewing tool (always available — vision fail-safe handled internally)
    // -------------------------------------------------------------------------
    registry.register(Arc::new(ViewImageTool::new(
        config.workspace_dir.clone(),
        config.vision_supported,
    )));
    registered_count += 1;

    info!(
        tool_count = registered_count,
        vision_supported = config.vision_supported,
        "Built-in tools registered"
    );

    registry
}
