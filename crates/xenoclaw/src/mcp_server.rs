//! MCP (Model Context Protocol) server implementation.
//!
//! Speaks JSON-RPC 2.0 over stdio, allowing Claude Code, Copilot, and other
//! MCP-compatible clients to connect and use XenoClaw's tools.
//!
//! Protocol: newline-delimited JSON on stdin/stdout.
//! Logging goes to stderr to avoid corrupting the transport.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use agent_core::ToolRegistry;

// ─── Error type ──────────────────────────────────────────────────────────────

/// Errors that can occur while running the MCP server.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

// ─── JSON-RPC types ──────────────────────────────────────────────────────────

/// An incoming JSON-RPC 2.0 message (request or notification).
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    /// Always "2.0".
    #[allow(dead_code)]
    jsonrpc: String,
    /// Request ID. Absent for notifications.
    id: Option<Value>,
    /// The method name.
    method: String,
    /// Method parameters.
    #[serde(default)]
    params: Value,
}

/// An outgoing JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// ─── JSON-RPC error codes ────────────────────────────────────────────────────

const PARSE_ERROR: i64 = -32700;
#[allow(dead_code)]
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
#[allow(dead_code)]
const INTERNAL_ERROR: i64 = -32603;

// ─── MCP Server ──────────────────────────────────────────────────────────────

/// The MCP protocol server. Wraps a ToolRegistry and handles JSON-RPC messages.
pub struct McpServer {
    registry: Arc<RwLock<ToolRegistry>>,
}

impl McpServer {
    /// Create a new MCP server backed by the given tool registry.
    pub fn new(registry: Arc<RwLock<ToolRegistry>>) -> Self {
        Self { registry }
    }

    /// Run the MCP server loop over stdio.
    ///
    /// Reads newline-delimited JSON from stdin and writes responses to stdout.
    /// Blocks until stdin is closed (EOF).
    pub async fn run_stdio(&self) -> Result<(), McpError> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        info!("MCP server started on stdio");

        while let Some(line) = lines.next_line().await? {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            debug!(raw = %line, "Received message");

            // Parse the JSON-RPC request
            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(e) => {
                    warn!(error = %e, "Failed to parse JSON-RPC message");
                    let response = JsonRpcResponse::error(
                        Value::Null,
                        PARSE_ERROR,
                        format!("Parse error: {e}"),
                    );
                    self.write_response(&mut stdout, &response).await?;
                    continue;
                }
            };

            // Notifications have no id and expect no response
            if request.id.is_none() {
                debug!(method = %request.method, "Received notification (no response needed)");
                continue;
            }

            let id = request.id.unwrap();
            let response = self
                .handle_request(&request.method, request.params, id.clone())
                .await;
            self.write_response(&mut stdout, &response).await?;
        }

        info!("MCP server stdin closed, shutting down");
        Ok(())
    }

    /// Write a JSON-RPC response as a single line to stdout.
    async fn write_response(
        &self,
        stdout: &mut tokio::io::Stdout,
        response: &JsonRpcResponse,
    ) -> Result<(), McpError> {
        let json = serde_json::to_string(response)?;
        debug!(raw = %json, "Sending response");
        stdout.write_all(json.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
        Ok(())
    }

    /// Dispatch a request to the appropriate handler.
    async fn handle_request(&self, method: &str, params: Value, id: Value) -> JsonRpcResponse {
        match method {
            "initialize" => self.handle_initialize(params, id),
            "tools/list" => self.handle_tools_list(id).await,
            "tools/call" => self.handle_tools_call(params, id).await,
            "resources/list" => self.handle_resources_list(id),
            "resources/read" => self.handle_resources_read(id),
            "prompts/list" => self.handle_prompts_list(id),
            "prompts/get" => self.handle_prompts_get(id),
            _ => {
                warn!(method = %method, "Unknown method");
                JsonRpcResponse::error(id, METHOD_NOT_FOUND, format!("Method not found: {method}"))
            }
        }
    }

    // ─── Method handlers ─────────────────────────────────────────────────────

    /// Handle `initialize` — MCP handshake.
    fn handle_initialize(&self, _params: Value, id: Value) -> JsonRpcResponse {
        info!("MCP client connected (initialize)");

        JsonRpcResponse::success(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "subscribe": false, "listChanged": false },
                    "prompts": { "listChanged": false }
                },
                "serverInfo": {
                    "name": "xenoclaw",
                    "version": "0.1.0"
                }
            }),
        )
    }

    /// Handle `tools/list` — return all registered tools with their schemas.
    async fn handle_tools_list(&self, id: Value) -> JsonRpcResponse {
        let registry = self.registry.read().await;
        // MCP clients drive their own approval flow, so expose every tool
        // (include_coding = true, plan_only = false).
        let definitions = registry.tool_definitions(true, false);

        let tools: Vec<Value> = definitions
            .iter()
            .map(|def| {
                json!({
                    "name": def.name,
                    "description": def.description,
                    "inputSchema": def.parameters
                })
            })
            .collect();

        JsonRpcResponse::success(id, json!({ "tools": tools }))
    }

    /// Handle `tools/call` — execute a tool and return the result.
    async fn handle_tools_call(&self, params: Value, id: Value) -> JsonRpcResponse {
        // Extract tool name
        let name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => {
                return JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    "Missing required parameter: name",
                );
            }
        };

        // Extract arguments (default to empty object)
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        info!(tool = %name, "Executing tool call");

        let registry = self.registry.read().await;

        // Check if tool exists
        let tool = match registry.get(&name) {
            Some(t) => Arc::clone(t),
            None => {
                return JsonRpcResponse::success(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": format!("Tool '{}' not found", name) }],
                        "isError": true
                    }),
                );
            }
        };

        // Execute the tool (drop the read lock first to avoid holding it during execution)
        drop(registry);

        match tool.execute(arguments).await {
            Ok(output) => JsonRpcResponse::success(
                id,
                json!({
                    "content": [{ "type": "text", "text": output }],
                    "isError": false
                }),
            ),
            Err(err) => {
                error!(tool = %name, error = %err, "Tool execution failed");
                JsonRpcResponse::success(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": err }],
                        "isError": true
                    }),
                )
            }
        }
    }

    /// Handle `resources/list` — placeholder, returns empty list.
    fn handle_resources_list(&self, id: Value) -> JsonRpcResponse {
        JsonRpcResponse::success(id, json!({ "resources": [] }))
    }

    /// Handle `resources/read` — not implemented yet.
    fn handle_resources_read(&self, id: Value) -> JsonRpcResponse {
        JsonRpcResponse::error(
            id,
            METHOD_NOT_FOUND,
            "resources/read is not implemented yet",
        )
    }

    /// Handle `prompts/list` — placeholder, returns empty list.
    fn handle_prompts_list(&self, id: Value) -> JsonRpcResponse {
        JsonRpcResponse::success(id, json!({ "prompts": [] }))
    }

    /// Handle `prompts/get` — not implemented yet.
    fn handle_prompts_get(&self, id: Value) -> JsonRpcResponse {
        JsonRpcResponse::error(id, METHOD_NOT_FOUND, "prompts/get is not implemented yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Tool;
    use async_trait::async_trait;

    /// A simple test tool for MCP server tests.
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echoes the input message back"
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Message to echo" }
                },
                "required": ["message"]
            })
        }

        async fn execute(&self, arguments: Value) -> Result<String, String> {
            let msg = arguments
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("no message");
            Ok(msg.to_string())
        }
    }

    fn make_registry() -> Arc<RwLock<ToolRegistry>> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        Arc::new(RwLock::new(registry))
    }

    #[tokio::test]
    async fn test_initialize() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1.0" }
                }),
                json!(1),
            )
            .await;

        let result = response.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "xenoclaw");
        assert_eq!(result["serverInfo"]["version"], "0.1.0");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn test_tools_list() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request("tools/list", json!({}), json!(2))
            .await;

        let result = response.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");
        assert_eq!(tools[0]["description"], "Echoes the input message back");
        assert!(tools[0]["inputSchema"].is_object());
    }

    #[tokio::test]
    async fn test_tools_call_success() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request(
                "tools/call",
                json!({ "name": "echo", "arguments": { "message": "hello" } }),
                json!(3),
            )
            .await;

        let result = response.result.unwrap();
        assert_eq!(result["isError"], false);
        let content = result["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "hello");
    }

    #[tokio::test]
    async fn test_tools_call_not_found() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request(
                "tools/call",
                json!({ "name": "nonexistent", "arguments": {} }),
                json!(4),
            )
            .await;

        let result = response.result.unwrap();
        assert_eq!(result["isError"], true);
        let content = result["content"].as_array().unwrap();
        assert!(content[0]["text"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_tools_call_missing_name() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request("tools/call", json!({}), json!(5))
            .await;

        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_resources_list() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request("resources/list", json!({}), json!(6))
            .await;

        let result = response.result.unwrap();
        assert_eq!(result["resources"], json!([]));
    }

    #[tokio::test]
    async fn test_resources_read_not_implemented() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request("resources/read", json!({}), json!(7))
            .await;

        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_prompts_list() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request("prompts/list", json!({}), json!(8))
            .await;

        let result = response.result.unwrap();
        assert_eq!(result["prompts"], json!([]));
    }

    #[tokio::test]
    async fn test_prompts_get_not_implemented() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request("prompts/get", json!({}), json!(9))
            .await;

        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_unknown_method() {
        let server = McpServer::new(make_registry());
        let response = server
            .handle_request("unknown/method", json!({}), json!(10))
            .await;

        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, METHOD_NOT_FOUND);
    }
}
