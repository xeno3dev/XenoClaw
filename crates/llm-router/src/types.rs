//! Request and response types for LLM provider communication.

use serde::{Deserialize, Serialize};

/// JSON key used to mark a tool result as carrying an inline image. The
/// `view_image` tool emits `{"__xeno_image__": {"media_type", "data", "note"}}`
/// and the agent core converts that into a multimodal `ChatMessage`.
pub const IMAGE_SENTINEL_KEY: &str = "__xeno_image__";

/// An inline image attached to a chat message (base64-encoded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    /// MIME type, e.g. "image/png", "image/jpeg".
    pub media_type: String,
    /// Base64-encoded image bytes (no `data:` URI prefix).
    pub data: String,
}

/// A message in a chat completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Inline images to send alongside the text (multimodal). Empty for
    /// text-only messages. Providers that don't support vision ignore this.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,
}

impl ChatMessage {
    /// Construct a text-only chat message.
    pub fn text(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            images: Vec::new(),
        }
    }

    /// Construct a chat message carrying one or more inline images.
    pub fn with_images(role: ChatRole, content: impl Into<String>, images: Vec<ImageContent>) -> Self {
        Self {
            role,
            content: content.into(),
            images,
        }
    }
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
