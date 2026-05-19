//! CLI-based LLM providers — delegates to external CLI tools.
//!
//! Supports:
//! - Claude Code CLI (`claude`) — uses Claude Pro/Max subscription
//! - GitHub Copilot CLI (`github-copilot`) — uses Copilot subscription
//!
//! These providers shell out to the respective CLI tools, passing the prompt
//! via stdin and capturing the response from stdout.

use std::process::Stdio;
use std::time::Duration;

use futures::stream::BoxStream;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, error};

use crate::types::{
    ChatMessage, CompletionChunk, CompletionRequest, CompletionResponse, TokenUsage,
};
use common::config::ProviderConfig;
use common::LlmError;

use super::LlmProvider;

/// Provider that delegates to the Claude Code CLI (`claude`).
///
/// Requires:
/// - Claude Code CLI installed (`claude` in PATH)
/// - Active Claude Pro or Max subscription
/// - Authenticated via `claude login`
pub struct ClaudeCodeProvider {
    name: String,
    timeout: Duration,
    model: String,
}

impl ClaudeCodeProvider {
    pub fn from_config(config: &ProviderConfig) -> Self {
        Self {
            name: config.name.clone(),
            timeout: Duration::from_secs(config.timeout_seconds as u64),
            model: config.model.clone(),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let prompt = build_prompt(&request.messages);

        let mut cmd = Command::new("claude");
        cmd.arg("--print")
            .arg("--model")
            .arg(&self.model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        debug!(provider = %self.name, model = %self.model, "Sending request to Claude Code CLI");

        let mut child = cmd.spawn().map_err(|e| LlmError::Timeout {
            provider: self.name.clone(),
            elapsed_ms: 0,
        })?;

        // Write prompt to stdin
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await.ok();
            drop(stdin);
        }

        // Wait for output with timeout
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| LlmError::Timeout {
                provider: self.name.clone(),
                elapsed_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(|e| LlmError::InvalidResponse {
                provider: self.name.clone(),
                reason: format!("CLI execution failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(provider = %self.name, stderr = %stderr, "Claude Code CLI returned error");
            return Err(LlmError::InvalidResponse {
                provider: self.name.clone(),
                reason: format!("CLI exited with status {}: {}", output.status, stderr.trim()),
            });
        }

        let content = String::from_utf8_lossy(&output.stdout).trim().to_string();

        Ok(CompletionResponse {
            content,
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: self.model.clone(),
        })
    }

    async fn complete_stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'static, Result<CompletionChunk, LlmError>>, LlmError> {
        // CLI providers don't support true streaming — fall back to full completion
        let response = self.complete(request).await?;
        let chunk = CompletionChunk {
            delta_content: Some(response.content),
            delta_tool_calls: vec![],
            done: true,
            usage: Some(response.usage),
        };
        Ok(Box::pin(futures::stream::once(async move { Ok(chunk) })))
    }
}

/// Provider that delegates to the GitHub Copilot CLI.
///
/// Requires:
/// - GitHub Copilot CLI installed (`github-copilot` in PATH)
/// - Active GitHub Copilot subscription
/// - Authenticated via `github-copilot auth`
pub struct CopilotCliProvider {
    name: String,
    timeout: Duration,
    model: String,
}

impl CopilotCliProvider {
    pub fn from_config(config: &ProviderConfig) -> Self {
        Self {
            name: config.name.clone(),
            timeout: Duration::from_secs(config.timeout_seconds as u64),
            model: config.model.clone(),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for CopilotCliProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let prompt = build_prompt(&request.messages);

        let mut cmd = Command::new("github-copilot");
        cmd.arg("--prompt")
            .arg(&prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        debug!(provider = %self.name, "Sending request to Copilot CLI");

        let output = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| LlmError::Timeout {
                provider: self.name.clone(),
                elapsed_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(|e| LlmError::InvalidResponse {
                provider: self.name.clone(),
                reason: format!("CLI execution failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(provider = %self.name, stderr = %stderr, "Copilot CLI returned error");
            return Err(LlmError::InvalidResponse {
                provider: self.name.clone(),
                reason: format!("CLI exited with status {}: {}", output.status, stderr.trim()),
            });
        }

        let content = String::from_utf8_lossy(&output.stdout).trim().to_string();

        Ok(CompletionResponse {
            content,
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: self.model.clone(),
        })
    }

    async fn complete_stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'static, Result<CompletionChunk, LlmError>>, LlmError> {
        // CLI providers don't support true streaming — fall back to full completion
        let response = self.complete(request).await?;
        let chunk = CompletionChunk {
            delta_content: Some(response.content),
            delta_tool_calls: vec![],
            done: true,
            usage: Some(response.usage),
        };
        Ok(Box::pin(futures::stream::once(async move { Ok(chunk) })))
    }
}

/// Build a plain-text prompt from chat messages for CLI tools.
fn build_prompt(messages: &[ChatMessage]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        let prefix = match msg.role {
            crate::types::ChatRole::System => "[System]",
            crate::types::ChatRole::User => "[User]",
            crate::types::ChatRole::Assistant => "[Assistant]",
            crate::types::ChatRole::Tool => "[Tool]",
        };
        parts.push(format!("{prefix} {}", msg.content));
    }
    parts.join("\n\n")
}
