//! Agent-specific types for the core runtime.
//!
//! Defines the agent's operational status, modes, response streaming,
//! and tool-related types used throughout the agent-core crate.

use std::path::PathBuf;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use common::errors::PlatformError;
use common::models::ToolResult;

/// The operational status of the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum AgentStatus {
    /// Agent is idle, waiting for work.
    Idle,
    /// Agent is actively processing a task.
    Working { task: String, progress: Option<f32> },
    /// Agent encountered an error.
    Error { message: String },
    /// Agent is shutting down.
    ShuttingDown,
    /// Agent is starting up.
    Starting,
}

impl Default for AgentStatus {
    fn default() -> Self {
        Self::Idle
    }
}

/// The operational mode of the agent, determining which tools are available.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AgentMode {
    /// General-purpose agent with base tools only.
    General,
    /// Coding agent with additional development tools.
    Coding { workspace: PathBuf },
}

impl Default for AgentMode {
    fn default() -> Self {
        Self::General
    }
}

/// A streamed response chunk from the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseChunk {
    /// Text content in this chunk (may be empty for tool-call-only chunks).
    pub content: Option<String>,
    /// Whether this is the final chunk.
    pub done: bool,
    /// Tool results included in this chunk (if any tools were executed).
    pub tool_results: Option<Vec<ToolResult>>,
}

/// A boxed stream of response chunks, representing the agent's response.
pub type ResponseStream = BoxStream<'static, Result<ResponseChunk, PlatformError>>;

/// Errors specific to agent mode switching.
#[derive(Debug, thiserror::Error)]
pub enum ModeError {
    /// The requested mode is not available (e.g., coding module not enabled).
    #[error("Mode not available: {reason}")]
    NotAvailable { reason: String },

    /// Cannot switch modes while processing a request.
    #[error("Cannot switch mode while agent is working")]
    AgentBusy,
}

/// Errors during agent shutdown.
#[derive(Debug, thiserror::Error)]
pub enum ShutdownError {
    /// Timed out waiting for in-flight requests to drain.
    #[error(
        "Shutdown timed out after {elapsed_seconds}s with {pending_requests} requests pending"
    )]
    Timeout {
        elapsed_seconds: u64,
        pending_requests: usize,
    },

    /// Failed to persist state before shutdown.
    #[error("Failed to save state: {reason}")]
    StatePersistFailed { reason: String },
}

/// Configuration for the agent core runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCoreConfig {
    /// Maximum number of tool call iterations per message (prevents infinite loops).
    pub max_tool_iterations: u32,
    /// Timeout in seconds for the entire message processing loop.
    pub message_timeout_seconds: u64,
    /// Graceful shutdown timeout in seconds.
    pub shutdown_timeout_seconds: u64,
    /// Maximum number of messages to load as context from history.
    pub context_history_limit: usize,
    /// System prompt to prepend to every CompletionRequest (if Some).
    #[serde(default)]
    pub system_prompt: Option<String>,
}

impl Default for AgentCoreConfig {
    fn default() -> Self {
        Self {
            max_tool_iterations: 20,
            message_timeout_seconds: 300,
            shutdown_timeout_seconds: 30,
            context_history_limit: 50,
            system_prompt: None,
        }
    }
}
