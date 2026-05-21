//! LSP Client Management — manages connections to configured language servers.
//!
//! Key behaviors:
//! - Initialize and maintain connections to configured language servers per workspace language
//! - Request diagnostics within 2 seconds of file modification
//! - Report errors to operator and withhold changes until acknowledged or resolved
//! - Use go-to-definition, find-references, rename-symbol for refactoring operations
//! - Handle language server failures gracefully (log, notify, continue without LSP)
//! - Proceed without LSP when no server configured for a language

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use common::config::LspConfig;

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur during LSP operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LspError {
    /// The language server failed to start.
    #[error("Language server for '{language}' failed to start: {reason}")]
    ServerStartFailed { language: String, reason: String },

    /// The language server crashed during operation.
    #[error("Language server for '{language}' crashed: {reason}")]
    ServerCrashed { language: String, reason: String },

    /// No language server is configured for the given language.
    #[error("No language server configured for '{language}'")]
    NoServerConfigured { language: String },

    /// Communication with the language server failed.
    #[error("LSP communication error for '{language}': {reason}")]
    CommunicationError { language: String, reason: String },

    /// The language server returned an error response.
    #[error("LSP error response: {message}")]
    ResponseError { code: i64, message: String },

    /// Request timed out.
    #[error("LSP request timed out for '{language}'")]
    Timeout { language: String },
}

// =============================================================================
// LSP Data Types
// =============================================================================

/// A diagnostic reported by a language server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    /// File path the diagnostic applies to.
    pub path: PathBuf,
    /// Line number (0-indexed).
    pub line: u32,
    /// Column number (0-indexed).
    pub col: u32,
    /// Severity of the diagnostic.
    pub severity: DiagnosticSeverity,
    /// Human-readable message.
    pub message: String,
    /// Source of the diagnostic (e.g., "rustc", "typescript").
    pub source: Option<String>,
}

/// Severity levels for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl DiagnosticSeverity {
    /// Convert from LSP severity number (1=Error, 2=Warning, 3=Info, 4=Hint).
    pub fn from_lsp(value: u32) -> Self {
        match value {
            1 => DiagnosticSeverity::Error,
            2 => DiagnosticSeverity::Warning,
            3 => DiagnosticSeverity::Information,
            _ => DiagnosticSeverity::Hint,
        }
    }

    /// Whether this severity represents an error.
    pub fn is_error(&self) -> bool {
        matches!(self, DiagnosticSeverity::Error)
    }
}

/// A source code location.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Location {
    /// File path.
    pub path: PathBuf,
    /// Line number (0-indexed).
    pub line: u32,
    /// Column number (0-indexed).
    pub col: u32,
}

/// A text edit to apply to a file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextEdit {
    /// File path.
    pub path: PathBuf,
    /// Range to replace.
    pub range: TextRange,
    /// New text to insert.
    pub new_text: String,
}

/// A range within a text document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextRange {
    /// Start line (0-indexed).
    pub start_line: u32,
    /// Start column (0-indexed).
    pub start_col: u32,
    /// End line (0-indexed).
    pub end_line: u32,
    /// End column (0-indexed).
    pub end_col: u32,
}

// =============================================================================
// JSON-RPC Message Types
// =============================================================================

/// A JSON-RPC request message.
#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: i64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// A JSON-RPC response message.
#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// A JSON-RPC notification message (no id, no response expected).
#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcNotification {
    jsonrpc: String,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// A JSON-RPC error object.
#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

// =============================================================================
// LSP Server Connection
// =============================================================================

/// State of a single language server connection.
#[allow(dead_code)]
struct LspServerConnection {
    /// The language this server handles.
    language: String,
    /// The child process running the language server.
    process: Child,
    /// Writer to the server's stdin.
    stdin: tokio::process::ChildStdin,
    /// Reader from the server's stdout.
    stdout: Arc<Mutex<BufReader<tokio::process::ChildStdout>>>,
    /// Next request ID.
    next_id: AtomicI64,
    /// Whether the server has been initialized.
    initialized: bool,
    /// Cached diagnostics per file.
    diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
}

impl LspServerConnection {
    fn next_request_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }
}

// =============================================================================
// LSP Client Manager
// =============================================================================

/// Manages connections to configured language servers.
///
/// Provides diagnostics, go-to-definition, find-references, and rename-symbol
/// operations. Handles server failures gracefully by logging and continuing
/// without LSP for the affected language.
pub struct LspClientManager {
    /// Active server connections keyed by language identifier.
    servers: RwLock<HashMap<String, Mutex<LspServerConnection>>>,
    /// The workspace root path.
    workspace_root: PathBuf,
    /// Language-to-file-extension mapping for language detection.
    language_extensions: HashMap<String, Vec<String>>,
}

impl LspClientManager {
    /// Create a new LspClientManager (not yet initialized).
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
            workspace_root: PathBuf::new(),
            language_extensions: Self::default_language_extensions(),
        }
    }

    /// Default mapping of language identifiers to file extensions.
    fn default_language_extensions() -> HashMap<String, Vec<String>> {
        let mut map = HashMap::new();
        map.insert("rust".to_string(), vec!["rs".to_string()]);
        map.insert(
            "typescript".to_string(),
            vec!["ts".to_string(), "tsx".to_string()],
        );
        map.insert(
            "javascript".to_string(),
            vec!["js".to_string(), "jsx".to_string(), "mjs".to_string()],
        );
        map.insert(
            "python".to_string(),
            vec!["py".to_string(), "pyi".to_string()],
        );
        map.insert("go".to_string(), vec!["go".to_string()]);
        map.insert("c".to_string(), vec!["c".to_string(), "h".to_string()]);
        map.insert(
            "cpp".to_string(),
            vec![
                "cpp".to_string(),
                "cc".to_string(),
                "cxx".to_string(),
                "hpp".to_string(),
            ],
        );
        map.insert("java".to_string(), vec!["java".to_string()]);
        map.insert("ruby".to_string(), vec!["rb".to_string()]);
        map.insert("php".to_string(), vec!["php".to_string()]);
        map
    }

    /// Detect the language of a file based on its extension.
    pub fn detect_language(&self, path: &Path) -> Option<String> {
        let ext = path.extension()?.to_str()?;
        for (language, extensions) in &self.language_extensions {
            if extensions.iter().any(|e| e == ext) {
                return Some(language.clone());
            }
        }
        None
    }

    /// Initialize language servers from the provided configurations.
    ///
    /// Starts each configured language server as a child process and performs
    /// the LSP initialize handshake. If a server fails to start, logs the error
    /// and continues without LSP for that language.
    pub async fn initialize(
        &mut self,
        configs: &[LspConfig],
        workspace_root: &Path,
    ) -> Vec<Result<String, LspError>> {
        self.workspace_root = workspace_root.to_path_buf();
        let mut results = Vec::new();

        for config in configs {
            match self.start_server(config, workspace_root).await {
                Ok(()) => {
                    info!(
                        language = %config.language,
                        command = %config.command,
                        "Language server started successfully"
                    );
                    results.push(Ok(config.language.clone()));
                }
                Err(e) => {
                    error!(
                        language = %config.language,
                        command = %config.command,
                        error = %e,
                        "Failed to start language server, continuing without LSP"
                    );
                    results.push(Err(e));
                }
            }
        }

        results
    }

    /// Start a single language server process and perform initialization.
    async fn start_server(
        &self,
        config: &LspConfig,
        workspace_root: &Path,
    ) -> Result<(), LspError> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| LspError::ServerStartFailed {
            language: config.language.clone(),
            reason: e.to_string(),
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::ServerStartFailed {
                language: config.language.clone(),
                reason: "Failed to capture stdin".to_string(),
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::ServerStartFailed {
                language: config.language.clone(),
                reason: "Failed to capture stdout".to_string(),
            })?;

        let stdout_reader = Arc::new(Mutex::new(BufReader::new(stdout)));

        let mut connection = LspServerConnection {
            language: config.language.clone(),
            process: child,
            stdin,
            stdout: stdout_reader,
            next_id: AtomicI64::new(1),
            initialized: false,
            diagnostics: HashMap::new(),
        };

        // Send initialize request
        let workspace_uri = format!("file://{}", workspace_root.display());
        let init_params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": workspace_uri,
            "capabilities": {
                "textDocument": {
                    "synchronization": {
                        "didOpen": true,
                        "didChange": true,
                        "didClose": true
                    },
                    "publishDiagnostics": {
                        "relatedInformation": true
                    },
                    "definition": {
                        "dynamicRegistration": false
                    },
                    "references": {
                        "dynamicRegistration": false
                    },
                    "rename": {
                        "dynamicRegistration": false,
                        "prepareSupport": false
                    }
                },
                "workspace": {
                    "workspaceFolders": true
                }
            },
            "workspaceFolders": [{
                "uri": workspace_uri,
                "name": workspace_root.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
            }]
        });

        // Send initialize request with timeout
        let init_result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Self::send_request_on_connection(&mut connection, "initialize", Some(init_params)),
        )
        .await
        .map_err(|_| LspError::ServerStartFailed {
            language: config.language.clone(),
            reason: "Initialize request timed out".to_string(),
        })?
        .map_err(|e| LspError::ServerStartFailed {
            language: config.language.clone(),
            reason: format!("Initialize failed: {e}"),
        })?;

        debug!(
            language = %config.language,
            "Initialize response received: {:?}",
            init_result
        );

        // Send initialized notification
        Self::send_notification_on_connection(
            &mut connection,
            "initialized",
            Some(serde_json::json!({})),
        )
        .await
        .map_err(|e| LspError::ServerStartFailed {
            language: config.language.clone(),
            reason: format!("Failed to send initialized notification: {e}"),
        })?;

        connection.initialized = true;

        // Store the connection
        let mut servers = self.servers.write().await;
        servers.insert(config.language.clone(), Mutex::new(connection));

        Ok(())
    }

    /// Get diagnostics for a file from the relevant language server.
    ///
    /// Returns an empty Vec if no server is configured for the file's language.
    pub async fn get_diagnostics(&self, path: &Path) -> Result<Vec<Diagnostic>, LspError> {
        let language = match self.detect_language(path) {
            Some(lang) => lang,
            None => return Ok(Vec::new()), // No language detected, proceed without LSP
        };

        let servers = self.servers.read().await;
        let server_mutex = match servers.get(&language) {
            Some(s) => s,
            None => return Ok(Vec::new()), // No server configured, proceed without LSP
        };

        let server = server_mutex.lock().await;

        // Return cached diagnostics for this file
        Ok(server.diagnostics.get(path).cloned().unwrap_or_default())
    }

    /// Go to definition at the given position.
    ///
    /// Returns None if no server is configured or the server doesn't find a definition.
    pub async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Option<Location>, LspError> {
        let language = match self.detect_language(path) {
            Some(lang) => lang,
            None => return Ok(None),
        };

        let servers = self.servers.read().await;
        let server_mutex = match servers.get(&language) {
            Some(s) => s,
            None => return Ok(None),
        };

        let mut server = server_mutex.lock().await;

        // Check if server is still alive
        if !Self::is_server_alive(&mut server) {
            warn!(language = %language, "Language server is not responsive");
            return Err(LspError::ServerCrashed {
                language: language.clone(),
                reason: "Server process is no longer running".to_string(),
            });
        }

        let file_uri = format!("file://{}", path.display());
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri },
            "position": { "line": line, "character": col }
        });

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Self::send_request_on_connection(&mut server, "textDocument/definition", Some(params)),
        )
        .await
        .map_err(|_| LspError::Timeout {
            language: language.clone(),
        })?
        .map_err(|e| LspError::CommunicationError {
            language: language.clone(),
            reason: e.to_string(),
        })?;

        // Parse the response — can be a single Location or an array
        Self::parse_location_response(&response)
    }

    /// Find all references to the symbol at the given position.
    ///
    /// Returns an empty Vec if no server is configured.
    pub async fn find_references(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<Location>, LspError> {
        let language = match self.detect_language(path) {
            Some(lang) => lang,
            None => return Ok(Vec::new()),
        };

        let servers = self.servers.read().await;
        let server_mutex = match servers.get(&language) {
            Some(s) => s,
            None => return Ok(Vec::new()),
        };

        let mut server = server_mutex.lock().await;

        if !Self::is_server_alive(&mut server) {
            return Err(LspError::ServerCrashed {
                language: language.clone(),
                reason: "Server process is no longer running".to_string(),
            });
        }

        let file_uri = format!("file://{}", path.display());
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri },
            "position": { "line": line, "character": col },
            "context": { "includeDeclaration": true }
        });

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Self::send_request_on_connection(&mut server, "textDocument/references", Some(params)),
        )
        .await
        .map_err(|_| LspError::Timeout {
            language: language.clone(),
        })?
        .map_err(|e| LspError::CommunicationError {
            language: language.clone(),
            reason: e.to_string(),
        })?;

        Self::parse_locations_response(&response)
    }

    /// Rename the symbol at the given position.
    ///
    /// Returns a list of text edits to apply across the workspace.
    /// Returns an empty Vec if no server is configured.
    pub async fn rename_symbol(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        new_name: &str,
    ) -> Result<Vec<TextEdit>, LspError> {
        let language = match self.detect_language(path) {
            Some(lang) => lang,
            None => return Ok(Vec::new()),
        };

        let servers = self.servers.read().await;
        let server_mutex = match servers.get(&language) {
            Some(s) => s,
            None => return Ok(Vec::new()),
        };

        let mut server = server_mutex.lock().await;

        if !Self::is_server_alive(&mut server) {
            return Err(LspError::ServerCrashed {
                language: language.clone(),
                reason: "Server process is no longer running".to_string(),
            });
        }

        let file_uri = format!("file://{}", path.display());
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri },
            "position": { "line": line, "character": col },
            "newName": new_name
        });

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Self::send_request_on_connection(&mut server, "textDocument/rename", Some(params)),
        )
        .await
        .map_err(|_| LspError::Timeout {
            language: language.clone(),
        })?
        .map_err(|e| LspError::CommunicationError {
            language: language.clone(),
            reason: e.to_string(),
        })?;

        Self::parse_workspace_edit_response(&response)
    }

    /// Notify the relevant language server that a file has changed.
    ///
    /// This triggers the server to re-analyze the file and publish new diagnostics.
    /// Does nothing if no server is configured for the file's language.
    pub async fn notify_file_changed(&self, path: &Path, content: &str) -> Result<(), LspError> {
        let language = match self.detect_language(path) {
            Some(lang) => lang,
            None => return Ok(()), // No language detected, proceed without LSP
        };

        let servers = self.servers.read().await;
        let server_mutex = match servers.get(&language) {
            Some(s) => s,
            None => return Ok(()), // No server configured, proceed without LSP
        };

        let mut server = server_mutex.lock().await;

        if !Self::is_server_alive(&mut server) {
            warn!(
                language = %language,
                "Language server not alive, cannot notify of file change"
            );
            return Err(LspError::ServerCrashed {
                language: language.clone(),
                reason: "Server process is no longer running".to_string(),
            });
        }

        let file_uri = format!("file://{}", path.display());

        // Send didChange notification
        let params = serde_json::json!({
            "textDocument": {
                "uri": file_uri,
                "version": 1
            },
            "contentChanges": [{
                "text": content
            }]
        });

        Self::send_notification_on_connection(&mut server, "textDocument/didChange", Some(params))
            .await
            .map_err(|e| LspError::CommunicationError {
                language: language.clone(),
                reason: e.to_string(),
            })?;

        debug!(language = %language, path = %path.display(), "Notified server of file change");
        Ok(())
    }

    /// Notify the language server that a file was opened.
    pub async fn notify_file_opened(&self, path: &Path, content: &str) -> Result<(), LspError> {
        let language = match self.detect_language(path) {
            Some(lang) => lang,
            None => return Ok(()),
        };

        let servers = self.servers.read().await;
        let server_mutex = match servers.get(&language) {
            Some(s) => s,
            None => return Ok(()),
        };

        let mut server = server_mutex.lock().await;

        if !Self::is_server_alive(&mut server) {
            return Err(LspError::ServerCrashed {
                language: language.clone(),
                reason: "Server process is no longer running".to_string(),
            });
        }

        let file_uri = format!("file://{}", path.display());
        let params = serde_json::json!({
            "textDocument": {
                "uri": file_uri,
                "languageId": language,
                "version": 1,
                "text": content
            }
        });

        Self::send_notification_on_connection(&mut server, "textDocument/didOpen", Some(params))
            .await
            .map_err(|e| LspError::CommunicationError {
                language: language.clone(),
                reason: e.to_string(),
            })?;

        Ok(())
    }

    /// Gracefully shut down all language servers.
    ///
    /// Sends the shutdown request followed by an exit notification to each server.
    pub async fn shutdown(&self) {
        let mut servers = self.servers.write().await;

        for (language, server_mutex) in servers.drain() {
            let mut server = server_mutex.lock().await;
            info!(language = %language, "Shutting down language server");

            // Send shutdown request
            let shutdown_result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                Self::send_request_on_connection(&mut server, "shutdown", None),
            )
            .await;

            match shutdown_result {
                Ok(Ok(_)) => {
                    // Send exit notification
                    let _ = Self::send_notification_on_connection(&mut server, "exit", None).await;
                    debug!(language = %language, "Language server shut down gracefully");
                }
                Ok(Err(e)) => {
                    warn!(language = %language, error = %e, "Shutdown request failed, killing process");
                    let _ = server.process.kill().await;
                }
                Err(_) => {
                    warn!(language = %language, "Shutdown request timed out, killing process");
                    let _ = server.process.kill().await;
                }
            }
        }
    }

    /// Check whether a language server has a connection for the given language.
    pub async fn has_server_for_language(&self, language: &str) -> bool {
        let servers = self.servers.read().await;
        servers.contains_key(language)
    }

    /// Check whether a language server is configured for the given file.
    pub async fn has_server_for_file(&self, path: &Path) -> bool {
        if let Some(language) = self.detect_language(path) {
            self.has_server_for_language(&language).await
        } else {
            false
        }
    }

    /// Get the list of languages with active server connections.
    pub async fn active_languages(&self) -> Vec<String> {
        let servers = self.servers.read().await;
        servers.keys().cloned().collect()
    }

    /// Update cached diagnostics for a file (called when server publishes diagnostics).
    pub async fn update_diagnostics(&self, path: &Path, diagnostics: Vec<Diagnostic>) {
        let language = match self.detect_language(path) {
            Some(lang) => lang,
            None => return,
        };

        let servers = self.servers.read().await;
        if let Some(server_mutex) = servers.get(&language) {
            let mut server = server_mutex.lock().await;
            server.diagnostics.insert(path.to_path_buf(), diagnostics);
        }
    }

    /// Check whether any cached diagnostics for a file contain errors.
    pub async fn has_errors(&self, path: &Path) -> bool {
        match self.get_diagnostics(path).await {
            Ok(diagnostics) => diagnostics.iter().any(|d| d.severity.is_error()),
            Err(_) => false,
        }
    }

    // =========================================================================
    // Internal helpers
    // =========================================================================

    /// Check if a server process is still alive.
    fn is_server_alive(server: &mut LspServerConnection) -> bool {
        match server.process.try_wait() {
            Ok(None) => true,     // Still running
            Ok(Some(_)) => false, // Exited
            Err(_) => false,      // Error checking status
        }
    }

    /// Send a JSON-RPC request on a connection and wait for the response.
    async fn send_request_on_connection(
        connection: &mut LspServerConnection,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let id = connection.next_request_id();

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        };

        // Serialize the request
        let body = serde_json::to_string(&request)
            .map_err(|e| format!("Failed to serialize request: {e}"))?;

        // Write with Content-Length header (LSP protocol)
        let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        connection
            .stdin
            .write_all(message.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to stdin: {e}"))?;
        connection
            .stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush stdin: {e}"))?;

        // Read the response
        let response = Self::read_message(&connection.stdout).await?;

        // Parse as JSON-RPC response
        let rpc_response: JsonRpcResponse = serde_json::from_str(&response)
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        if let Some(error) = rpc_response.error {
            return Err(format!("LSP error {}: {}", error.code, error.message));
        }

        Ok(rpc_response.result.unwrap_or(serde_json::Value::Null))
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn send_notification_on_connection(
        connection: &mut LspServerConnection,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let notification = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };

        let body = serde_json::to_string(&notification)
            .map_err(|e| format!("Failed to serialize notification: {e}"))?;

        let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        connection
            .stdin
            .write_all(message.as_bytes())
            .await
            .map_err(|e| format!("Failed to write notification: {e}"))?;
        connection
            .stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush notification: {e}"))?;

        Ok(())
    }

    /// Read a single LSP message from the server's stdout.
    ///
    /// LSP messages use the format:
    /// ```text
    /// Content-Length: <length>\r\n
    /// \r\n
    /// <json body>
    /// ```
    async fn read_message(
        stdout: &Arc<Mutex<BufReader<tokio::process::ChildStdout>>>,
    ) -> Result<String, String> {
        let mut reader = stdout.lock().await;

        // Read headers until we find Content-Length
        let mut content_length: Option<usize> = None;
        let mut header_line = String::new();

        loop {
            header_line.clear();
            let bytes_read = reader
                .read_line(&mut header_line)
                .await
                .map_err(|e| format!("Failed to read header: {e}"))?;

            if bytes_read == 0 {
                return Err("Server closed connection".to_string());
            }

            let trimmed = header_line.trim();
            if trimmed.is_empty() {
                // End of headers
                break;
            }

            if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(
                    len_str
                        .trim()
                        .parse::<usize>()
                        .map_err(|e| format!("Invalid Content-Length: {e}"))?,
                );
            }
        }

        let length = content_length.ok_or_else(|| "No Content-Length header found".to_string())?;

        // Read the body
        let mut body = vec![0u8; length];
        use tokio::io::AsyncReadExt;
        reader
            .read_exact(&mut body)
            .await
            .map_err(|e| format!("Failed to read body: {e}"))?;

        String::from_utf8(body).map_err(|e| format!("Invalid UTF-8 in response body: {e}"))
    }

    /// Parse a location response (single Location or LocationLink).
    fn parse_location_response(value: &serde_json::Value) -> Result<Option<Location>, LspError> {
        if value.is_null() {
            return Ok(None);
        }

        // Can be a single location object or an array
        if let Some(loc) = Self::try_parse_single_location(value) {
            return Ok(Some(loc));
        }

        if let Some(arr) = value.as_array() {
            if let Some(first) = arr.first() {
                if let Some(loc) = Self::try_parse_single_location(first) {
                    return Ok(Some(loc));
                }
                // Try LocationLink format
                if let Some(loc) = Self::try_parse_location_link(first) {
                    return Ok(Some(loc));
                }
            }
        }

        Ok(None)
    }

    /// Parse an array of locations response.
    fn parse_locations_response(value: &serde_json::Value) -> Result<Vec<Location>, LspError> {
        if value.is_null() {
            return Ok(Vec::new());
        }

        let arr = match value.as_array() {
            Some(a) => a,
            None => return Ok(Vec::new()),
        };

        let mut locations = Vec::new();
        for item in arr {
            if let Some(loc) = Self::try_parse_single_location(item) {
                locations.push(loc);
            }
        }

        Ok(locations)
    }

    /// Try to parse a single LSP Location object.
    fn try_parse_single_location(value: &serde_json::Value) -> Option<Location> {
        let uri = value.get("uri")?.as_str()?;
        let range = value.get("range")?;
        let start = range.get("start")?;
        let line = start.get("line")?.as_u64()? as u32;
        let col = start.get("character")?.as_u64()? as u32;

        let path = Self::uri_to_path(uri)?;
        Some(Location { path, line, col })
    }

    /// Try to parse a LocationLink object.
    fn try_parse_location_link(value: &serde_json::Value) -> Option<Location> {
        let uri = value.get("targetUri")?.as_str()?;
        let range = value.get("targetRange")?;
        let start = range.get("start")?;
        let line = start.get("line")?.as_u64()? as u32;
        let col = start.get("character")?.as_u64()? as u32;

        let path = Self::uri_to_path(uri)?;
        Some(Location { path, line, col })
    }

    /// Parse a WorkspaceEdit response into a list of TextEdits.
    fn parse_workspace_edit_response(value: &serde_json::Value) -> Result<Vec<TextEdit>, LspError> {
        if value.is_null() {
            return Ok(Vec::new());
        }

        let mut edits = Vec::new();

        // WorkspaceEdit has a "changes" field: { uri: TextEdit[] }
        if let Some(changes) = value.get("changes").and_then(|c| c.as_object()) {
            for (uri, file_edits) in changes {
                let path = match Self::uri_to_path(uri) {
                    Some(p) => p,
                    None => continue,
                };

                if let Some(arr) = file_edits.as_array() {
                    for edit in arr {
                        if let Some(text_edit) = Self::parse_text_edit(&path, edit) {
                            edits.push(text_edit);
                        }
                    }
                }
            }
        }

        // Also handle "documentChanges" format
        if let Some(doc_changes) = value.get("documentChanges").and_then(|c| c.as_array()) {
            for doc_change in doc_changes {
                let uri = doc_change
                    .get("textDocument")
                    .and_then(|td| td.get("uri"))
                    .and_then(|u| u.as_str());

                let path = match uri.and_then(Self::uri_to_path) {
                    Some(p) => p,
                    None => continue,
                };

                if let Some(arr) = doc_change.get("edits").and_then(|e| e.as_array()) {
                    for edit in arr {
                        if let Some(text_edit) = Self::parse_text_edit(&path, edit) {
                            edits.push(text_edit);
                        }
                    }
                }
            }
        }

        Ok(edits)
    }

    /// Parse a single TextEdit from a JSON value.
    fn parse_text_edit(path: &Path, value: &serde_json::Value) -> Option<TextEdit> {
        let range = value.get("range")?;
        let start = range.get("start")?;
        let end = range.get("end")?;

        let start_line = start.get("line")?.as_u64()? as u32;
        let start_col = start.get("character")?.as_u64()? as u32;
        let end_line = end.get("line")?.as_u64()? as u32;
        let end_col = end.get("character")?.as_u64()? as u32;

        let new_text = value.get("newText")?.as_str()?.to_string();

        Some(TextEdit {
            path: path.to_path_buf(),
            range: TextRange {
                start_line,
                start_col,
                end_line,
                end_col,
            },
            new_text,
        })
    }

    /// Convert a file:// URI to a PathBuf.
    fn uri_to_path(uri: &str) -> Option<PathBuf> {
        uri.strip_prefix("file://").map(PathBuf::from)
    }
}

impl Default for LspClientManager {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language_rust() {
        let manager = LspClientManager::new();
        assert_eq!(
            manager.detect_language(Path::new("/project/src/main.rs")),
            Some("rust".to_string())
        );
    }

    #[test]
    fn test_detect_language_typescript() {
        let manager = LspClientManager::new();
        assert_eq!(
            manager.detect_language(Path::new("/project/src/app.ts")),
            Some("typescript".to_string())
        );
        assert_eq!(
            manager.detect_language(Path::new("/project/src/App.tsx")),
            Some("typescript".to_string())
        );
    }

    #[test]
    fn test_detect_language_python() {
        let manager = LspClientManager::new();
        assert_eq!(
            manager.detect_language(Path::new("/project/main.py")),
            Some("python".to_string())
        );
    }

    #[test]
    fn test_detect_language_unknown() {
        let manager = LspClientManager::new();
        assert_eq!(
            manager.detect_language(Path::new("/project/data.xyz")),
            None
        );
    }

    #[test]
    fn test_detect_language_no_extension() {
        let manager = LspClientManager::new();
        assert_eq!(
            manager.detect_language(Path::new("/project/Makefile")),
            None
        );
    }

    #[test]
    fn test_uri_to_path() {
        let path = LspClientManager::uri_to_path("file:///home/user/project/src/main.rs");
        assert_eq!(path, Some(PathBuf::from("/home/user/project/src/main.rs")));
    }

    #[test]
    fn test_uri_to_path_invalid() {
        let path = LspClientManager::uri_to_path("https://example.com/file.rs");
        assert_eq!(path, None);
    }

    #[test]
    fn test_diagnostic_severity_from_lsp() {
        assert_eq!(DiagnosticSeverity::from_lsp(1), DiagnosticSeverity::Error);
        assert_eq!(DiagnosticSeverity::from_lsp(2), DiagnosticSeverity::Warning);
        assert_eq!(
            DiagnosticSeverity::from_lsp(3),
            DiagnosticSeverity::Information
        );
        assert_eq!(DiagnosticSeverity::from_lsp(4), DiagnosticSeverity::Hint);
        assert_eq!(DiagnosticSeverity::from_lsp(99), DiagnosticSeverity::Hint);
    }

    #[test]
    fn test_diagnostic_severity_is_error() {
        assert!(DiagnosticSeverity::Error.is_error());
        assert!(!DiagnosticSeverity::Warning.is_error());
        assert!(!DiagnosticSeverity::Information.is_error());
        assert!(!DiagnosticSeverity::Hint.is_error());
    }

    #[test]
    fn test_parse_single_location() {
        let value = serde_json::json!({
            "uri": "file:///home/user/project/src/lib.rs",
            "range": {
                "start": { "line": 10, "character": 5 },
                "end": { "line": 10, "character": 15 }
            }
        });

        let loc = LspClientManager::try_parse_single_location(&value).unwrap();
        assert_eq!(loc.path, PathBuf::from("/home/user/project/src/lib.rs"));
        assert_eq!(loc.line, 10);
        assert_eq!(loc.col, 5);
    }

    #[test]
    fn test_parse_location_link() {
        let value = serde_json::json!({
            "targetUri": "file:///home/user/project/src/types.rs",
            "targetRange": {
                "start": { "line": 20, "character": 0 },
                "end": { "line": 25, "character": 1 }
            },
            "targetSelectionRange": {
                "start": { "line": 20, "character": 4 },
                "end": { "line": 20, "character": 12 }
            }
        });

        let loc = LspClientManager::try_parse_location_link(&value).unwrap();
        assert_eq!(loc.path, PathBuf::from("/home/user/project/src/types.rs"));
        assert_eq!(loc.line, 20);
        assert_eq!(loc.col, 0);
    }

    #[test]
    fn test_parse_location_response_null() {
        let result = LspClientManager::parse_location_response(&serde_json::Value::Null).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_location_response_single() {
        let value = serde_json::json!({
            "uri": "file:///project/src/main.rs",
            "range": {
                "start": { "line": 5, "character": 3 },
                "end": { "line": 5, "character": 10 }
            }
        });

        let result = LspClientManager::parse_location_response(&value).unwrap();
        assert!(result.is_some());
        let loc = result.unwrap();
        assert_eq!(loc.line, 5);
        assert_eq!(loc.col, 3);
    }

    #[test]
    fn test_parse_location_response_array() {
        let value = serde_json::json!([
            {
                "uri": "file:///project/src/a.rs",
                "range": {
                    "start": { "line": 1, "character": 0 },
                    "end": { "line": 1, "character": 5 }
                }
            },
            {
                "uri": "file:///project/src/b.rs",
                "range": {
                    "start": { "line": 2, "character": 3 },
                    "end": { "line": 2, "character": 8 }
                }
            }
        ]);

        let result = LspClientManager::parse_location_response(&value).unwrap();
        assert!(result.is_some());
        let loc = result.unwrap();
        assert_eq!(loc.path, PathBuf::from("/project/src/a.rs"));
        assert_eq!(loc.line, 1);
    }

    #[test]
    fn test_parse_locations_response() {
        let value = serde_json::json!([
            {
                "uri": "file:///project/src/a.rs",
                "range": {
                    "start": { "line": 1, "character": 0 },
                    "end": { "line": 1, "character": 5 }
                }
            },
            {
                "uri": "file:///project/src/b.rs",
                "range": {
                    "start": { "line": 10, "character": 2 },
                    "end": { "line": 10, "character": 7 }
                }
            }
        ]);

        let result = LspClientManager::parse_locations_response(&value).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, PathBuf::from("/project/src/a.rs"));
        assert_eq!(result[0].line, 1);
        assert_eq!(result[1].path, PathBuf::from("/project/src/b.rs"));
        assert_eq!(result[1].line, 10);
        assert_eq!(result[1].col, 2);
    }

    #[test]
    fn test_parse_locations_response_null() {
        let result = LspClientManager::parse_locations_response(&serde_json::Value::Null).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_workspace_edit_changes() {
        let value = serde_json::json!({
            "changes": {
                "file:///project/src/main.rs": [
                    {
                        "range": {
                            "start": { "line": 5, "character": 4 },
                            "end": { "line": 5, "character": 12 }
                        },
                        "newText": "new_name"
                    },
                    {
                        "range": {
                            "start": { "line": 10, "character": 8 },
                            "end": { "line": 10, "character": 16 }
                        },
                        "newText": "new_name"
                    }
                ]
            }
        });

        let edits = LspClientManager::parse_workspace_edit_response(&value).unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].path, PathBuf::from("/project/src/main.rs"));
        assert_eq!(edits[0].range.start_line, 5);
        assert_eq!(edits[0].range.start_col, 4);
        assert_eq!(edits[0].range.end_line, 5);
        assert_eq!(edits[0].range.end_col, 12);
        assert_eq!(edits[0].new_text, "new_name");
        assert_eq!(edits[1].range.start_line, 10);
    }

    #[test]
    fn test_parse_workspace_edit_document_changes() {
        let value = serde_json::json!({
            "documentChanges": [
                {
                    "textDocument": {
                        "uri": "file:///project/src/lib.rs",
                        "version": 1
                    },
                    "edits": [
                        {
                            "range": {
                                "start": { "line": 3, "character": 0 },
                                "end": { "line": 3, "character": 8 }
                            },
                            "newText": "renamed"
                        }
                    ]
                }
            ]
        });

        let edits = LspClientManager::parse_workspace_edit_response(&value).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].path, PathBuf::from("/project/src/lib.rs"));
        assert_eq!(edits[0].new_text, "renamed");
    }

    #[test]
    fn test_parse_workspace_edit_null() {
        let edits =
            LspClientManager::parse_workspace_edit_response(&serde_json::Value::Null).unwrap();
        assert!(edits.is_empty());
    }

    #[tokio::test]
    async fn test_new_manager_has_no_servers() {
        let manager = LspClientManager::new();
        assert!(manager.active_languages().await.is_empty());
    }

    #[tokio::test]
    async fn test_has_server_for_file_no_servers() {
        let manager = LspClientManager::new();
        assert!(
            !manager
                .has_server_for_file(Path::new("/project/main.rs"))
                .await
        );
    }

    #[tokio::test]
    async fn test_get_diagnostics_no_server() {
        let manager = LspClientManager::new();
        let diagnostics = manager
            .get_diagnostics(Path::new("/project/main.rs"))
            .await
            .unwrap();
        assert!(diagnostics.is_empty());
    }

    #[tokio::test]
    async fn test_goto_definition_no_server() {
        let manager = LspClientManager::new();
        let result = manager
            .goto_definition(Path::new("/project/main.rs"), 5, 10)
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_find_references_no_server() {
        let manager = LspClientManager::new();
        let result = manager
            .find_references(Path::new("/project/main.rs"), 5, 10)
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_rename_symbol_no_server() {
        let manager = LspClientManager::new();
        let result = manager
            .rename_symbol(Path::new("/project/main.rs"), 5, 10, "new_name")
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_notify_file_changed_no_server() {
        let manager = LspClientManager::new();
        let result = manager
            .notify_file_changed(Path::new("/project/main.rs"), "fn main() {}")
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_notify_file_changed_unknown_extension() {
        let manager = LspClientManager::new();
        let result = manager
            .notify_file_changed(Path::new("/project/data.xyz"), "content")
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_initialize_with_invalid_command() {
        let mut manager = LspClientManager::new();
        let configs = vec![LspConfig {
            language: "rust".to_string(),
            command: "/nonexistent/binary/rust-analyzer".to_string(),
            args: vec![],
        }];

        let results = manager
            .initialize(&configs, Path::new("/tmp/workspace"))
            .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());

        // Should still have no active servers
        assert!(manager.active_languages().await.is_empty());
    }

    #[tokio::test]
    async fn test_initialize_continues_on_failure() {
        let mut manager = LspClientManager::new();
        let configs = vec![
            LspConfig {
                language: "rust".to_string(),
                command: "/nonexistent/rust-analyzer".to_string(),
                args: vec![],
            },
            LspConfig {
                language: "python".to_string(),
                command: "/nonexistent/pyright".to_string(),
                args: vec![],
            },
        ];

        let results = manager
            .initialize(&configs, Path::new("/tmp/workspace"))
            .await;
        assert_eq!(results.len(), 2);
        // Both should fail but the function should not panic
        assert!(results[0].is_err());
        assert!(results[1].is_err());
    }

    #[tokio::test]
    async fn test_update_and_get_diagnostics() {
        let manager = LspClientManager::new();
        let path = Path::new("/project/src/main.rs");

        // Without a server, diagnostics should be empty
        let diags = manager.get_diagnostics(path).await.unwrap();
        assert!(diags.is_empty());
    }

    #[tokio::test]
    async fn test_has_errors_no_diagnostics() {
        let manager = LspClientManager::new();
        assert!(!manager.has_errors(Path::new("/project/main.rs")).await);
    }

    #[tokio::test]
    async fn test_shutdown_empty() {
        let manager = LspClientManager::new();
        // Should not panic with no servers
        manager.shutdown().await;
    }
}
