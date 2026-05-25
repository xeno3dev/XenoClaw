//! Anthropic Claude API client.
//!
//! Implements the Anthropic Messages API (POST /v1/messages).

use std::time::Duration;

use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::LlmProvider;
use crate::types::*;
use common::config::ProviderConfig;
use common::LlmError;

/// API version header value for Anthropic.
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Client for the Anthropic Messages API.
pub struct AnthropicProvider {
    name: String,
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
    timeout: Duration,
    max_tokens_default: u32,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider from a `ProviderConfig`.
    ///
    /// # Panics
    /// Panics if `config.api_key` is `None` (Anthropic requires an API key).
    pub fn from_config(config: &ProviderConfig) -> Self {
        let api_key = config
            .api_key
            .clone()
            .expect("Anthropic provider requires an API key");
        Self::new(
            &config.name,
            &config.base_url,
            api_key,
            &config.model,
            config.timeout_seconds,
            config.max_tokens,
        )
    }

    /// Create a new Anthropic provider client.
    ///
    /// # Arguments
    /// * `name` - Display name for this provider
    /// * `base_url` - Base URL (e.g., "https://api.anthropic.com")
    /// * `api_key` - Anthropic API key
    /// * `model` - Model identifier (e.g., "claude-sonnet-4-20250514")
    /// * `timeout_seconds` - Request timeout (clamped to 5–120s, default 30s)
    /// * `max_tokens_default` - Default max tokens if not specified in request
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout_seconds: u32,
        max_tokens_default: Option<u32>,
    ) -> Self {
        let timeout_seconds = timeout_seconds.clamp(5, 120);
        let timeout = Duration::from_secs(timeout_seconds as u64);

        let client = Client::builder()
            .timeout(timeout)
            .build()
            .expect("failed to build HTTP client");

        Self {
            name: name.into(),
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            model: model.into(),
            timeout,
            max_tokens_default: max_tokens_default.unwrap_or(4096),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }

    fn build_request_body(&self, request: &CompletionRequest, stream: bool) -> AnthropicRequest {
        // Anthropic separates system messages from the conversation
        let mut system_prompt = None;
        let mut messages: Vec<AnthropicMessage> = Vec::new();

        for msg in &request.messages {
            match msg.role {
                ChatRole::System => {
                    system_prompt = Some(msg.content.clone());
                }
                ChatRole::User | ChatRole::Tool => {
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: anthropic_content(msg),
                    });
                }
                ChatRole::Assistant => {
                    messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: anthropic_content(msg),
                    });
                }
            }
        }

        let tools = if request.tools.is_empty() {
            None
        } else {
            Some(
                request
                    .tools
                    .iter()
                    .map(|t| AnthropicTool {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        input_schema: t.parameters.clone(),
                    })
                    .collect(),
            )
        };

        let max_tokens = request.max_tokens.unwrap_or(self.max_tokens_default);

        AnthropicRequest {
            model: self.model.clone(),
            messages,
            system: system_prompt,
            tools,
            max_tokens,
            temperature: request.temperature,
            stream,
        }
    }
}

/// Build the Anthropic `content` field: a plain string when there are no
/// images, or an array of text+image blocks when the message carries images.
fn anthropic_content(msg: &ChatMessage) -> serde_json::Value {
    if msg.images.is_empty() {
        return serde_json::Value::String(msg.content.clone());
    }
    let mut blocks: Vec<serde_json::Value> = Vec::new();
    if !msg.content.is_empty() {
        blocks.push(serde_json::json!({ "type": "text", "text": msg.content }));
    }
    for img in &msg.images {
        blocks.push(serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": img.media_type,
                "data": img.data,
            }
        }));
    }
    serde_json::Value::Array(blocks)
}

/// Anthropic vision is supported by Claude 3+ (opus/sonnet/haiku) and Claude 4.x.
/// Legacy claude-2/claude-instant are text-only.
fn anthropic_model_supports_vision(model: &str) -> bool {
    let m = model.to_lowercase();
    if m.contains("claude-2") || m.contains("claude-instant") {
        return false;
    }
    m.contains("claude-3")
        || m.contains("claude-opus")
        || m.contains("claude-sonnet")
        || m.contains("claude-haiku")
        || m.contains("claude-4")
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn supports_vision(&self) -> bool {
        anthropic_model_supports_vision(&self.model)
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let start = std::time::Instant::now();
        let body = self.build_request_body(request, false);

        debug!(provider = %self.name, model = %self.model, "Sending Anthropic completion request");

        let response = self
            .client
            .post(self.endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                let elapsed = start.elapsed().as_millis() as u64;
                if e.is_timeout() {
                    LlmError::Timeout {
                        provider: self.name.clone(),
                        elapsed_ms: elapsed,
                    }
                } else {
                    LlmError::InvalidResponse {
                        provider: self.name.clone(),
                        reason: format!("HTTP request failed: {e}"),
                    }
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(LlmError::RateLimited {
                    provider: self.name.clone(),
                    retry_after: Duration::from_secs(60),
                });
            }
            return Err(LlmError::InvalidResponse {
                provider: self.name.clone(),
                reason: format!("HTTP {status}: {body_text}"),
            });
        }

        let anthropic_response: AnthropicResponse =
            response
                .json()
                .await
                .map_err(|e| LlmError::InvalidResponse {
                    provider: self.name.clone(),
                    reason: format!("Failed to parse response: {e}"),
                })?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();

        for block in anthropic_response.content {
            match block {
                ContentBlock::Text { text } => {
                    content.push_str(&text);
                }
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(LlmToolCall {
                        id,
                        name,
                        arguments: serde_json::to_string(&input).unwrap_or_default(),
                    });
                }
            }
        }

        Ok(CompletionResponse {
            content,
            tool_calls,
            usage: TokenUsage {
                prompt_tokens: anthropic_response.usage.input_tokens,
                completion_tokens: anthropic_response.usage.output_tokens,
                total_tokens: anthropic_response.usage.input_tokens
                    + anthropic_response.usage.output_tokens,
            },
            model: anthropic_response.model,
        })
    }

    async fn complete_stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'static, Result<CompletionChunk, LlmError>>, LlmError> {
        let start = std::time::Instant::now();
        let body = self.build_request_body(request, true);

        debug!(provider = %self.name, model = %self.model, "Sending Anthropic streaming request");

        let response = self
            .client
            .post(self.endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                let elapsed = start.elapsed().as_millis() as u64;
                if e.is_timeout() {
                    LlmError::Timeout {
                        provider: self.name.clone(),
                        elapsed_ms: elapsed,
                    }
                } else {
                    LlmError::InvalidResponse {
                        provider: self.name.clone(),
                        reason: format!("HTTP request failed: {e}"),
                    }
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(LlmError::InvalidResponse {
                provider: self.name.clone(),
                reason: format!("HTTP {status}: {body_text}"),
            });
        }

        let provider_name = self.name.clone();
        let byte_stream = response.bytes_stream();

        let stream = byte_stream
            .map(move |chunk_result| match chunk_result {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    parse_anthropic_sse(&text, &provider_name)
                }
                Err(e) => vec![Err(LlmError::InvalidResponse {
                    provider: provider_name.clone(),
                    reason: format!("Stream error: {e}"),
                })],
            })
            .flat_map(futures::stream::iter);

        Ok(Box::pin(stream))
    }
}

/// Parse Anthropic SSE events into completion chunks.
fn parse_anthropic_sse(text: &str, provider_name: &str) -> Vec<Result<CompletionChunk, LlmError>> {
    let mut results = Vec::new();
    let mut current_event_type = String::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            current_event_type.clear();
            continue;
        }

        if let Some(event_type) = line.strip_prefix("event: ") {
            current_event_type = event_type.to_string();
            continue;
        }

        if let Some(data) = line.strip_prefix("data: ") {
            match current_event_type.as_str() {
                "content_block_delta" => {
                    if let Ok(delta) = serde_json::from_str::<AnthropicDeltaEvent>(data) {
                        match delta.delta {
                            AnthropicDelta::TextDelta { text } => {
                                results.push(Ok(CompletionChunk {
                                    delta_content: Some(text),
                                    delta_tool_calls: vec![],
                                    done: false,
                                    usage: None,
                                }));
                            }
                            AnthropicDelta::InputJsonDelta { partial_json } => {
                                results.push(Ok(CompletionChunk {
                                    delta_content: None,
                                    delta_tool_calls: vec![LlmToolCall {
                                        id: String::new(),
                                        name: String::new(),
                                        arguments: partial_json,
                                    }],
                                    done: false,
                                    usage: None,
                                }));
                            }
                        }
                    }
                }
                "message_delta" => {
                    if let Ok(msg_delta) = serde_json::from_str::<AnthropicMessageDelta>(data) {
                        results.push(Ok(CompletionChunk {
                            delta_content: None,
                            delta_tool_calls: vec![],
                            done: msg_delta.delta.stop_reason.is_some(),
                            usage: msg_delta.usage.map(|u| TokenUsage {
                                prompt_tokens: 0,
                                completion_tokens: u.output_tokens,
                                total_tokens: u.output_tokens,
                            }),
                        }));
                    }
                }
                "message_stop" => {
                    results.push(Ok(CompletionChunk {
                        delta_content: None,
                        delta_tool_calls: vec![],
                        done: true,
                        usage: None,
                    }));
                }
                _ => {
                    // Ignore other event types (message_start, content_block_start, etc.)
                }
            }
        }
    }

    results
}

// =============================================================================
// Anthropic API wire types (internal)
// =============================================================================

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    /// Either a plain string or an array of content blocks (text + image).
    content: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    model: String,
    content: Vec<ContentBlock>,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// Streaming types
#[derive(Debug, Deserialize)]
struct AnthropicDeltaEvent {
    delta: AnthropicDelta,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    delta: AnthropicMessageDeltaInner,
    usage: Option<AnthropicDeltaUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDeltaInner {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDeltaUsage {
    output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeout_clamping() {
        let provider = AnthropicProvider::new(
            "test",
            "http://localhost",
            "key",
            "claude-sonnet-4-20250514",
            3,
            None,
        );
        assert_eq!(provider.timeout, Duration::from_secs(5));

        let provider = AnthropicProvider::new(
            "test",
            "http://localhost",
            "key",
            "claude-sonnet-4-20250514",
            200,
            None,
        );
        assert_eq!(provider.timeout, Duration::from_secs(120));
    }

    #[test]
    fn test_endpoint_construction() {
        let provider = AnthropicProvider::new(
            "test",
            "https://api.anthropic.com",
            "key",
            "claude-sonnet-4-20250514",
            30,
            None,
        );
        assert_eq!(provider.endpoint(), "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn test_request_body_system_separation() {
        let provider = AnthropicProvider::new(
            "test",
            "http://localhost",
            "key",
            "claude-sonnet-4-20250514",
            30,
            None,
        );

        let request = CompletionRequest {
            messages: vec![
                ChatMessage::text(ChatRole::System, "You are helpful."),
                ChatMessage::text(ChatRole::User, "Hello"),
            ],
            tools: vec![],
            max_tokens: Some(2000),
            temperature: Some(0.5),
            stream: false,
        };

        let body = provider.build_request_body(&request, false);
        assert_eq!(body.system, Some("You are helpful.".to_string()));
        assert_eq!(body.messages.len(), 1);
        assert_eq!(body.messages[0].role, "user");
        assert_eq!(body.messages[0].content, serde_json::json!("Hello"));
        assert_eq!(body.max_tokens, 2000);
    }

    #[test]
    fn test_image_message_builds_content_blocks() {
        let provider = AnthropicProvider::new(
            "test",
            "http://localhost",
            "key",
            "claude-sonnet-4-20250514",
            30,
            None,
        );
        assert!(provider.supports_vision());

        let request = CompletionRequest {
            messages: vec![ChatMessage::with_images(
                ChatRole::User,
                "look",
                vec![ImageContent {
                    media_type: "image/png".to_string(),
                    data: "AAAA".to_string(),
                }],
            )],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            stream: false,
        };
        let body = provider.build_request_body(&request, false);
        let content = &body.messages[0].content;
        assert!(content.is_array());
        let arr = content.as_array().unwrap();
        assert_eq!(arr.len(), 2); // text + image
        assert_eq!(arr[1]["type"], "image");
        assert_eq!(arr[1]["source"]["media_type"], "image/png");
    }

    #[test]
    fn test_request_body_default_max_tokens() {
        let provider = AnthropicProvider::new(
            "test",
            "http://localhost",
            "key",
            "claude-sonnet-4-20250514",
            30,
            Some(8192),
        );

        let request = CompletionRequest {
            messages: vec![ChatMessage::text(ChatRole::User, "Hi")],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            stream: false,
        };

        let body = provider.build_request_body(&request, false);
        assert_eq!(body.max_tokens, 8192);
    }

    #[test]
    fn test_parse_anthropic_sse_text_delta() {
        let text = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n";
        let results = parse_anthropic_sse(text, "test");
        assert_eq!(results.len(), 1);
        let chunk = results[0].as_ref().unwrap();
        assert_eq!(chunk.delta_content.as_deref(), Some("Hello"));
        assert!(!chunk.done);
    }

    #[test]
    fn test_parse_anthropic_sse_message_stop() {
        let text = "event: message_stop\ndata: {}\n\n";
        let results = parse_anthropic_sse(text, "test");
        assert_eq!(results.len(), 1);
        let chunk = results[0].as_ref().unwrap();
        assert!(chunk.done);
    }
}
