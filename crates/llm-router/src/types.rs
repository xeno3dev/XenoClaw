//! Request and response types for LLM provider communication.

use serde::{Deserialize, Serialize};

/// A message in a chat completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

/// Role of a chat message participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool definition that the LLM can invoke.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A completion request sent to an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// The conversation messages.
    pub messages: Vec<ChatMessage>,
    /// Available tools the model can call.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Maximum tokens to generate (provider default if None).
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0–2.0).
    pub temperature: Option<f32>,
    /// Whether to stream the response.
    #[serde(default)]
    pub stream: bool,
}

/// A tool call returned by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Token usage statistics for a completion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// A complete (non-streaming) response from an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// The generated text content.
    pub content: String,
    /// Any tool calls the model wants to make.
    #[serde(default)]
    pub tool_calls: Vec<LlmToolCall>,
    /// Token usage statistics.
    pub usage: TokenUsage,
    /// The model that generated this response.
    pub model: String,
}

/// A single chunk in a streaming completion response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionChunk {
    /// Incremental text content (may be empty for tool-call-only chunks).
    pub delta_content: Option<String>,
    /// Incremental tool call data.
    pub delta_tool_calls: Vec<LlmToolCall>,
    /// Whether this is the final chunk.
    pub done: bool,
    /// Usage stats (typically only present in the final chunk).
    pub usage: Option<TokenUsage>,
}
