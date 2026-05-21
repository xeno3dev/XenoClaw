//! MCP Client — connects to external MCP servers as child processes.
//!
//! For each configured MCP server, this module:
//! 1. Spawns the server as a child process
//! 2. Performs the MCP initialize handshake over stdin/stdout
//! 3. Discovers available tools via `tools/list`
//! 4. Registers proxy tools in the ToolRegistry that forward `tools/call` requests
//!
//! Communication uses newline-delimited JSON-RPC over the child's stdin/stdout.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::RwLock;
use tracing::{info, warn};

use agent_core::{Tool, ToolRegistry};
use common::config::McpServerConfig;

/// Manages connections to external MCP servers.
pub struct McpClientManager {
    connections: Vec<Arc<RwLock<McpConnection>>>,
    children: Vec<Child>,
}

/// A connection to a single external MCP server (stdin/stdout handles + state).
struct McpConnection {
    name: String,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpConnection {
    /// Send a JSON-RPC request and wait for the response.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        self.send_message(&request).await?;
        self.read_response(id).await
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn notify(&mut self, method: &str) -> Result<(), String> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        self.send_message(&notification).await
    }

    /// Write a JSON message followed by a newline to the child's stdin.
    async fn send_message(&mut self, message: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(message)
            .map_err(|e| format!("Failed to serialize JSON-RPC message: {e}"))?;
        line.push('\n');

        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to MCP server '{}': {e}", self.name))?;

        self.stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush stdin for MCP server '{}': {e}", self.name))?;

        Ok(())
    }

    /// Read lines from stdout until we get a response matching the given id.
    async fn read_response(&mut self, expected_id: u64) -> Result<Value, String> {
        loop {
            let mut line = String::new();
            let bytes_read = self
                .stdout
                .read_line(&mut line)
                .await
                .map_err(|e| format!("Failed to read from MCP server '{}': {e}", self.name))?;

            if bytes_read == 0 {
                return Err(format!(
                    "MCP server '{}' closed stdout unexpectedly",
                    self.name
                ));
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let msg: Value = serde_json::from_str(line).map_err(|e| {
                format!(
                    "Invalid JSON from MCP server '{}': {e} (line: {line})",
                    self.name
                )
            })?;

            // Check if this is the response we're waiting for
            if let Some(id) = msg.get("id") {
                let msg_id = id.as_u64().unwrap_or(u64::MAX);
                if msg_id == expected_id {
                    // Check for error
                    if let Some(error) = msg.get("error") {
                        let error_msg = error
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        return Err(format!(
                            "MCP server '{}' returned error: {error_msg}",
                            self.name
                        ));
                    }
                    return Ok(msg);
                }
            }
            // If it's a notification or a response for a different id, skip it
        }
    }
}

/// A proxy tool that forwards `tools/call` requests to an external MCP server.
struct McpProxyTool {
    tool_name: String,
    tool_description: String,
    input_schema: Value,
    connection: Arc<RwLock<McpConnection>>,
}

#[async_trait]
impl Tool for McpProxyTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters_schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let mut conn = self.connection.write().await;

        let params = json!({
            "name": self.tool_name,
            "arguments": arguments,
        });

        let response = conn.request("tools/call", params).await?;

        // Extract the result content
        let result = response
            .get("result")
            .ok_or_else(|| "MCP tool call response missing 'result' field".to_string())?;

        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Concatenate all text content blocks
        let content = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        if is_error {
            Err(content)
        } else {
            Ok(content)
        }
    }
}

/// Connect to all configured MCP servers and register their tools.
///
/// Skips disabled servers and servers that fail to start or initialize.
/// Returns the `McpClientManager` which holds the child processes and should
/// be kept alive for the duration of the agent runtime.
pub async fn connect_mcp_servers(
    configs: &[McpServerConfig],
    registry: &mut ToolRegistry,
) -> McpClientManager {
    let mut manager = McpClientManager {
        connections: Vec::new(),
        children: Vec::new(),
    };

    for config in configs {
        if config.disabled {
            info!(server = %config.name, "MCP server disabled, skipping");
            continue;
        }

        match spawn_and_initialize(config).await {
            Ok((child, connection)) => {
                let connection = Arc::new(RwLock::new(connection));

                // Discover tools
                match discover_tools(&connection).await {
                    Ok(tools) => {
                        let tool_count = tools.len();
                        for tool_def in tools {
                            let tool_name = tool_def
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();

                            if tool_name.is_empty() {
                                warn!(
                                    server = %config.name,
                                    "Skipping tool with empty name"
                                );
                                continue;
                            }

                            let tool_description = tool_def
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string();

                            let input_schema = tool_def
                                .get("inputSchema")
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "object"}));

                            let proxy = Arc::new(McpProxyTool {
                                tool_name: tool_name.clone(),
                                tool_description,
                                input_schema,
                                connection: Arc::clone(&connection),
                            });

                            if !registry.register(proxy) {
                                warn!(
                                    server = %config.name,
                                    tool = %tool_name,
                                    "Tool name conflict, skipping duplicate"
                                );
                            }
                        }

                        info!(
                            server = %config.name,
                            tools = tool_count,
                            "MCP server connected and tools registered"
                        );

                        manager.connections.push(connection);
                        manager.children.push(child);
                    }
                    Err(e) => {
                        warn!(
                            server = %config.name,
                            error = %e,
                            "Failed to discover tools from MCP server, skipping"
                        );
                        // Drop the child — it will be killed
                    }
                }
            }
            Err(e) => {
                warn!(
                    server = %config.name,
                    error = %e,
                    "Failed to start MCP server, skipping"
                );
            }
        }
    }

    manager
}

/// Spawn a child process for an MCP server and perform the initialize handshake.
async fn spawn_and_initialize(config: &McpServerConfig) -> Result<(Child, McpConnection), String> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    cmd.envs(&config.env);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn MCP server '{}': {e}", config.name))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("Failed to capture stdin for MCP server '{}'", config.name))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("Failed to capture stdout for MCP server '{}'", config.name))?;

    let mut connection = McpConnection {
        name: config.name.clone(),
        stdin: BufWriter::new(stdin),
        stdout: BufReader::new(stdout),
        next_id: 1,
    };

    // Send initialize request
    let init_params = json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {
            "name": "xenoclaw",
            "version": "0.1.0"
        }
    });

    let _init_response = connection.request("initialize", init_params).await?;

    // Send initialized notification
    connection.notify("notifications/initialized").await?;

    Ok((child, connection))
}

/// Discover tools from a connected MCP server via `tools/list`.
async fn discover_tools(connection: &Arc<RwLock<McpConnection>>) -> Result<Vec<Value>, String> {
    let mut conn = connection.write().await;
    let response = conn.request("tools/list", json!({})).await?;

    let tools = response
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(tools)
}

impl McpClientManager {
    /// Gracefully shut down all connected MCP servers.
    ///
    /// Sends SIGTERM (on Unix) or kills (on Windows) each child process.
    pub async fn shutdown(&mut self) {
        for child in &mut self.children {
            // Try to get the child's id for logging
            let pid = child.id().unwrap_or(0);

            #[cfg(unix)]
            {
                // Send SIGTERM for graceful shutdown
                if let Some(id) = child.id() {
                    unsafe {
                        libc::kill(id as i32, libc::SIGTERM);
                    }
                }
            }

            #[cfg(not(unix))]
            {
                let _ = child.kill().await;
            }

            // Give the process a moment to exit, then force kill
            match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                Ok(Ok(status)) => {
                    info!(pid = pid, status = %status, "MCP server process exited");
                }
                Ok(Err(e)) => {
                    warn!(pid = pid, error = %e, "Error waiting for MCP server process");
                }
                Err(_) => {
                    warn!(
                        pid = pid,
                        "MCP server process did not exit in time, killing"
                    );
                    let _ = child.kill().await;
                }
            }
        }

        self.children.clear();
        self.connections.clear();
        info!("All MCP client connections shut down");
    }

    /// Check if any child processes have exited unexpectedly and log warnings.
    #[allow(dead_code)]
    pub async fn check_health(&mut self) {
        for child in &mut self.children {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let pid = child.id().unwrap_or(0);
                    warn!(
                        pid = pid,
                        status = %status,
                        "MCP server process exited unexpectedly"
                    );
                }
                Ok(None) => {
                    // Still running, good
                }
                Err(e) => {
                    warn!(error = %e, "Failed to check MCP server process status");
                }
            }
        }
    }
}
