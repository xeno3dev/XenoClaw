pub mod http;
pub mod ws;

use std::fmt;

use crate::app::{AgentStatus, ConnectionState};
use serde::{Deserialize, Serialize};

/// Sent from the UI to the backend client tasks.
#[derive(Debug, Clone)]
pub enum ClientCommand {
    /// User submitted a message — POST it.
    SendMessage { content: String },
    /// User pressed Ctrl-C during generation — try to cancel server-side.
    CancelGeneration,
    /// Force a reconnect (e.g., from a slash command).
    Reconnect,
    /// Graceful shutdown.
    Shutdown,
}

/// Sent from backend tasks to the UI.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// A token or chunk of the assistant's streamed response.
    AssistantToken { content: String },
    /// Assistant finished a response (closes the current message).
    AssistantDone,
    /// A tool call was initiated.
    ToolCall { name: String, args_summary: String },
    /// A tool finished.
    ToolResult {
        name: String,
        success: bool,
        summary: String,
    },
    /// Periodic status update.
    Status {
        status: AgentStatus,
        cpu: f32,
        memory_mb: f32,
        active_tasks: Vec<TaskSummary>,
    },
    /// Connection state change.
    Connection(ConnectionState),
    /// A protocol or network error to surface to the user.
    Error { message: String },
}

/// Summary of an active task for the status display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub name: String,
    pub status: String,
}

/// Error type for client operations.
#[derive(Debug)]
pub enum ClientError {
    Http { status: u16, body: String },
    Network { message: String },
    Auth { message: String },
    Protocol { message: String },
    Timeout,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Http { status, body } => write!(f, "HTTP {status}: {body}"),
            ClientError::Network { message } => write!(f, "Network: {message}"),
            ClientError::Auth { message } => write!(f, "Auth: {message}"),
            ClientError::Protocol { message } => write!(f, "Protocol: {message}"),
            ClientError::Timeout => write!(f, "Request timed out"),
        }
    }
}

impl std::error::Error for ClientError {}
