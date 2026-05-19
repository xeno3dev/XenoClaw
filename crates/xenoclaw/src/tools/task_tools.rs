//! Task Tools — Tool trait implementations wrapping `task_scheduler::Scheduler`.
//!
//! Provides:
//! - `task_create` — create a new scheduled task
//! - `task_list` — list all registered tasks with their next run time

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use tracing::debug;

use agent_core::tool_registry::Tool;
use common::models::{FsEvent, HttpMethod, RetryPolicy, Task, TaskAction, TaskStatus, TaskTrigger};
use common::types::TaskId;
use task_scheduler::Scheduler;

// =============================================================================
// TaskCreateTool
// =============================================================================

/// Tool that creates a new scheduled task.
pub struct TaskCreateTool {
    scheduler: Arc<Scheduler>,
}

impl TaskCreateTool {
    /// Create a new TaskCreateTool wrapping the given Scheduler.
    pub fn new(scheduler: Arc<Scheduler>) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &str {
        "task_create"
    }

    fn description(&self) -> &str {
        "Create a new scheduled task with a cron expression or other trigger type. \
         Supports cron, file_change, webhook, and time_condition triggers."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Human-readable name for the task"
                },
                "trigger_type": {
                    "type": "string",
                    "enum": ["cron", "file_change", "webhook", "time_condition"],
                    "description": "The type of trigger for this task"
                },
                "trigger_expression": {
                    "type": "string",
                    "description": "The trigger expression (cron expression, file glob, webhook path, or time condition)"
                },
                "command": {
                    "type": "string",
                    "description": "The command to execute when the task fires"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments for the command (optional)"
                },
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Task timeout in seconds (default: 300)",
                    "default": 300
                }
            },
            "required": ["name", "trigger_type", "trigger_expression", "command"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'name'".to_string())?;

        let trigger_type = arguments
            .get("trigger_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'trigger_type'".to_string())?;

        let trigger_expression = arguments
            .get("trigger_expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'trigger_expression'".to_string())?;

        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'command'".to_string())?;

        let args: Vec<String> = arguments
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let timeout_seconds = arguments
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(300) as u32;

        debug!(
            name = %name,
            trigger_type = %trigger_type,
            "Executing task_create tool"
        );

        // Build the trigger
        let trigger = match trigger_type {
            "cron" => TaskTrigger::Cron {
                expression: trigger_expression.to_string(),
            },
            "file_change" => TaskTrigger::FileChange {
                paths: vec![PathBuf::from(trigger_expression)],
                events: vec![FsEvent::Modified, FsEvent::Created],
            },
            "webhook" => TaskTrigger::Webhook {
                path: trigger_expression.to_string(),
                method: HttpMethod::Post,
            },
            "time_condition" => TaskTrigger::TimeCondition {
                expression: trigger_expression.to_string(),
            },
            _ => {
                return Err(format!(
                    "Invalid trigger_type '{}'. Must be one of: cron, file_change, webhook, time_condition",
                    trigger_type
                ));
            }
        };

        let now = Utc::now();
        let task = Task {
            id: TaskId::new(),
            name: name.to_string(),
            trigger,
            action: TaskAction::Command {
                command: command.to_string(),
                args,
            },
            timeout_seconds,
            dependencies: vec![],
            retry_policy: RetryPolicy::default(),
            status: TaskStatus::Active,
            created_at: now,
            updated_at: now,
        };

        let task_id = self
            .scheduler
            .create_task(task)
            .await
            .map_err(|e| e.to_string())?;

        let response = json!({
            "task_id": task_id.0.to_string(),
            "name": name,
            "created": true
        });

        Ok(response.to_string())
    }
}

// =============================================================================
// TaskListTool
// =============================================================================

/// Tool that lists all registered tasks with their status and next run time.
pub struct TaskListTool {
    scheduler: Arc<Scheduler>,
}

impl TaskListTool {
    /// Create a new TaskListTool wrapping the given Scheduler.
    pub fn new(scheduler: Arc<Scheduler>) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &str {
        "task_list"
    }

    fn description(&self) -> &str {
        "List all registered scheduled tasks with their trigger type and next run time."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _arguments: Value) -> Result<String, String> {
        debug!("Executing task_list tool");

        let tasks = self.scheduler.list_tasks().await;

        let entries: Vec<Value> = tasks
            .iter()
            .map(|t| {
                json!({
                    "id": t.id.0.to_string(),
                    "name": t.name,
                    "trigger_type": t.trigger_type,
                    "next_run": t.next_run.map(|dt| dt.to_rfc3339())
                })
            })
            .collect();

        let response = json!({
            "tasks": entries,
            "count": entries.len()
        });

        Ok(response.to_string())
    }
}
