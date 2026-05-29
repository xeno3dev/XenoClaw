//! Shell Command Tool — implements the `Tool` trait for shell command execution.
//!
//! This tool is marked as `coding_only() -> true` and provides shell command
//! execution capabilities through the Tool Registry.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

use agent_core::tool_registry::Tool;

use crate::shell_executor::{CommandOutput, ShellExecutor};

/// A tool that executes shell commands, registered with the Tool Registry.
///
/// Accepts command, args, cwd (optional), timeout (optional).
/// Validates command against allowlist/blocklist.
/// Returns JSON with stdout, stderr, exit_code.
/// Marked as `coding_only() -> true`.
pub struct ShellCommandTool {
    executor: ShellExecutor,
}

impl ShellCommandTool {
    /// Create a new ShellCommandTool wrapping the given executor.
    pub fn new(executor: ShellExecutor) -> Self {
        Self { executor }
    }

    /// Get a reference to the underlying executor.
    pub fn executor(&self) -> &ShellExecutor {
        &self.executor
    }
}

#[async_trait]
impl Tool for ShellCommandTool {
    fn name(&self) -> &str {
        "shell_execute"
    }

    fn description(&self) -> &str {
        "Execute a shell command on the VPS, capturing stdout, stderr, and exit code. \
         Commands are validated against the configured allowlist/blocklist. \
         Supports configurable timeout and working directory."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute (must be on the allowlist)"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments to pass to the command"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command (optional)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (optional, default: 300)"
                },
                "env": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "Environment variables to set for the command (optional)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        // Parse arguments
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

        let cwd = arguments
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);

        let timeout = arguments
            .get("timeout")
            .and_then(|v| v.as_u64())
            .map(Duration::from_secs);

        let env: Option<HashMap<String, String>> =
            arguments.get("env").and_then(|v| v.as_object()).map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            });

        debug!(
            command = %command,
            args = ?args,
            cwd = ?cwd,
            timeout = ?timeout,
            "Executing shell command tool"
        );

        // Execute the command
        let result = self
            .executor
            .execute(command, &args, cwd.as_ref(), env.as_ref(), timeout)
            .await;

        match result {
            Ok(output) => {
                let response = format_output(&output);
                Ok(response)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn coding_only(&self) -> bool {
        true
    }

    fn is_destructive(&self) -> bool {
        true
    }
}

/// Format a CommandOutput as a JSON string for the tool result.
fn format_output(output: &CommandOutput) -> String {
    let json = json!({
        "stdout": output.stdout,
        "stderr": output.stderr,
        "exit_code": output.exit_code,
        "timed_out": output.timed_out,
        "duration_ms": output.duration_ms,
    });
    json.to_string()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_executor::ShellExecutorConfig;

    fn test_tool() -> ShellCommandTool {
        let config = ShellExecutorConfig {
            command_allowlist: vec![
                "echo".to_string(),
                "sh".to_string(),
                "true".to_string(),
                "false".to_string(),
            ],
            command_blocklist: vec!["rm".to_string()],
            default_timeout_seconds: 30,
            max_concurrent_shells: 5,
        };
        ShellCommandTool::new(ShellExecutor::new(config))
    }

    #[test]
    fn test_tool_metadata() {
        let tool = test_tool();
        assert_eq!(tool.name(), "shell_execute");
        assert!(tool.coding_only());
        assert!(!tool.description().is_empty());

        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["command"].is_object());
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("command")));
    }

    #[tokio::test]
    async fn test_tool_execute_success() {
        let tool = test_tool();
        let args = json!({
            "command": "echo",
            "args": ["hello from tool"]
        });

        let result = tool.execute(args).await;
        assert!(result.is_ok());

        let output: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(output["stdout"].as_str().unwrap().trim(), "hello from tool");
        assert_eq!(output["exit_code"], 0);
        assert_eq!(output["timed_out"], false);
    }

    #[tokio::test]
    async fn test_tool_execute_denied_command() {
        let tool = test_tool();
        let args = json!({
            "command": "rm",
            "args": ["-rf", "/"]
        });

        let result = tool.execute(args).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("denied"));
    }

    #[tokio::test]
    async fn test_tool_execute_missing_command() {
        let tool = test_tool();
        let args = json!({
            "args": ["hello"]
        });

        let result = tool.execute(args).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required parameter"));
    }

    #[tokio::test]
    async fn test_tool_execute_with_cwd() {
        let tool = test_tool();
        let args = json!({
            "command": "sh",
            "args": ["-c", "pwd"],
            "cwd": "/tmp"
        });

        let result = tool.execute(args).await;
        assert!(result.is_ok());

        let output: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(output["stdout"].as_str().unwrap().contains("tmp"));
    }

    #[tokio::test]
    async fn test_tool_execute_captures_exit_code() {
        let tool = test_tool();
        let args = json!({
            "command": "false"
        });

        let result = tool.execute(args).await;
        assert!(result.is_ok());

        let output: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(output["exit_code"], 1);
    }

    #[tokio::test]
    async fn test_tool_execute_with_env() {
        let tool = test_tool();
        let args = json!({
            "command": "sh",
            "args": ["-c", "echo $TEST_VAR"],
            "env": { "TEST_VAR": "tool_env_value" }
        });

        let result = tool.execute(args).await;
        assert!(result.is_ok());

        let output: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(output["stdout"].as_str().unwrap().trim(), "tool_env_value");
    }

    #[tokio::test]
    async fn test_format_output_json() {
        let output = CommandOutput {
            stdout: "hello\n".to_string(),
            stderr: "".to_string(),
            exit_code: Some(0),
            timed_out: false,
            duration_ms: 42,
        };

        let json_str = format_output(&output);
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["stdout"], "hello\n");
        assert_eq!(parsed["stderr"], "");
        assert_eq!(parsed["exit_code"], 0);
        assert_eq!(parsed["timed_out"], false);
        assert_eq!(parsed["duration_ms"], 42);
    }
}
