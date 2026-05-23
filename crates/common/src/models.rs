//! Core data models for the VPS AI Agent Platform.
//!
//! This module defines the shared data structures used across all crates,
//! including messaging, task scheduling, and user/auth models.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{ApiKeyId, MessageId, SessionId, TaskId, UserId};

// =============================================================================
// Messaging Models
// =============================================================================

/// A chat message exchanged between user, assistant, system, or tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub session_id: SessionId,
    pub role: MessageRole,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_results: Option<Vec<ToolResult>>,
    pub timestamp: DateTime<Utc>,
    pub token_count: u32,
}

/// The role of a message sender in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

/// A tool invocation requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// The result of executing a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub output: String,
    pub is_error: bool,
}

// =============================================================================
// Task Models
// =============================================================================

/// A scheduled or triggered task definition with its current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    pub trigger: TaskTrigger,
    pub action: TaskAction,
    pub timeout_seconds: u32,
    pub dependencies: Vec<TaskId>,
    pub retry_policy: RetryPolicy,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The current lifecycle status of a task definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TaskStatus {
    Active,
    Paused,
    Failed { last_error: String },
}

/// A single execution record of a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: uuid::Uuid,
    pub task_id: TaskId,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub status: TaskRunStatus,
    pub error: Option<String>,
    pub attempt: u8,
}

/// The outcome status of a single task execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskRunStatus {
    Running,
    Succeeded,
    Failed,
    TimedOut,
}

/// What a task does when triggered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TaskAction {
    Prompt {
        template: String,
        tools: Vec<String>,
    },
    Command {
        command: String,
        args: Vec<String>,
    },
    Webhook {
        url: String,
        method: HttpMethod,
        body: Option<String>,
    },
}

/// What causes a task to execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskTrigger {
    Cron {
        expression: String,
    },
    FileChange {
        paths: Vec<PathBuf>,
        events: Vec<FsEvent>,
    },
    Webhook {
        path: String,
        method: HttpMethod,
    },
    TimeCondition {
        expression: String,
    },
    TaskCompletion {
        task_id: TaskId,
    },
}

/// Retry policy for failed task executions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (default: 3).
    pub max_retries: u8,
    /// Base interval in seconds for exponential backoff (default: 10).
    pub base_interval_secs: u32,
    /// Maximum interval cap in seconds (default: 300).
    pub max_interval_secs: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_interval_secs: 10,
            max_interval_secs: 300,
        }
    }
}

/// HTTP methods supported by webhook triggers and actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// Filesystem events that can trigger a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsEvent {
    Created,
    Modified,
    Deleted,
}

// =============================================================================
// User and Auth Models
// =============================================================================

/// A platform user account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub password_hash: String,
    pub role: Role,
    pub api_keys: Vec<ApiKey>,
    pub messaging_identities: Vec<MessagingIdentity>,
    pub created_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
}

/// An API key associated with a user for programmatic access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: ApiKeyId,
    /// The key is stored as a hash, never in plaintext.
    pub key_hash: String,
    pub name: String,
    /// Maximum requests per minute allowed for this key.
    pub rate_limit: u32,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

/// A user's identity on an external messaging platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagingIdentity {
    pub platform: Platform,
    pub platform_user_id: String,
    pub verified: bool,
}

/// An active user session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub user_id: UserId,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub token: String,
    pub mode: AgentMode,
}

/// Authorization roles for platform users.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Operator,
}

/// Supported external messaging platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Telegram,
    Discord,
    WhatsApp,
}

/// The operational mode of the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AgentMode {
    General,
    Coding {
        workspace: PathBuf,
        #[serde(default)]
        plan_only: bool,
    },
}

impl Default for AgentMode {
    fn default() -> Self {
        Self::General
    }
}

// =============================================================================
// Access Control Types (used by Security Layer)
// =============================================================================

/// Types of filesystem access that can be granted or denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessType {
    Read,
    Write,
    Execute,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_role_serialization() {
        let role = MessageRole::Assistant;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"assistant\"");
        let deserialized: MessageRole = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, role);
    }

    #[test]
    fn test_task_status_serialization() {
        let status = TaskStatus::Failed {
            last_error: "timeout".to_string(),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"type\":\"failed\""));
        assert!(json.contains("\"last_error\":\"timeout\""));
    }

    #[test]
    fn test_task_run_status_serialization() {
        let status = TaskRunStatus::Succeeded;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"succeeded\"");
    }

    #[test]
    fn test_task_action_prompt_serialization() {
        let action = TaskAction::Prompt {
            template: "Hello {{name}}".to_string(),
            tools: vec!["search".to_string()],
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("\"type\":\"prompt\""));
    }

    #[test]
    fn test_task_trigger_cron_serialization() {
        let trigger = TaskTrigger::Cron {
            expression: "0 * * * *".to_string(),
        };
        let json = serde_json::to_string(&trigger).unwrap();
        assert!(json.contains("\"type\":\"cron\""));
    }

    #[test]
    fn test_retry_policy_defaults() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.base_interval_secs, 10);
        assert_eq!(policy.max_interval_secs, 300);
    }

    #[test]
    fn test_role_serialization() {
        let role = Role::Admin;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"admin\"");
        let deserialized: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, role);
    }

    #[test]
    fn test_platform_serialization() {
        let platform = Platform::Telegram;
        let json = serde_json::to_string(&platform).unwrap();
        assert_eq!(json, "\"telegram\"");
    }

    #[test]
    fn test_agent_mode_default() {
        let mode = AgentMode::default();
        matches!(mode, AgentMode::General);
    }

    #[test]
    fn test_access_type_serialization() {
        let access = AccessType::Write;
        let json = serde_json::to_string(&access).unwrap();
        assert_eq!(json, "\"write\"");
    }

    #[test]
    fn test_tool_call_serialization() {
        let tool_call = ToolCall {
            id: "call_123".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"query": "test"}),
        };
        let json = serde_json::to_string(&tool_call).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "call_123");
        assert_eq!(deserialized.name, "search");
    }

    #[test]
    fn test_tool_result_serialization() {
        let result = ToolResult {
            tool_call_id: "call_123".to_string(),
            output: "found 5 results".to_string(),
            is_error: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ToolResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tool_call_id, "call_123");
        assert!(!deserialized.is_error);
    }
}
