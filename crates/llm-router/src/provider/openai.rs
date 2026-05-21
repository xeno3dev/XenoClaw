//! OpenAI-compatible API client.
//!
//! Supports any API that implements the OpenAI chat completions interface
//! (OpenAI, Together AI, vLLM, LM Studio, etc.)

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

/// Client for OpenAI-compatible chat completion APIs.
pub struct OpenAiProvider {
    name: String,
    client: Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
    timeout: Duration,
}

impl OpenAiProvider {
    /// Create a new OpenAI-compatible provider from a `ProviderConfig`.
    pub fn from_config(config: &ProviderConfig) -> Self {
        Self::new(
            &config.name,
            &config.base_url,
            config.api_key.clone(),
            &config.model,
            config.timeout_seconds,
        )
    }

    /// Create a new OpenAI-compatible provider client.
    ///
    /// # Arguments
    /// * `name` - Display name for this provider
    /// * `base_url` - Base URL (e.g., "https://api.openai.com")
    /// * `api_key` - Optional API key for authentication
    /// * `model` - Model identifier (e.g., "gpt-4o")
    /// * `timeout_seconds` - Request timeout (clamped to 5–120s, default 30s)
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: impl Into<String>,
        timeout_seconds: u32,
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
            api_key,
            model: model.into(),
            timeout,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }

    fn build_request_body(&self, request: &CompletionRequest) -> OpenAiRequest {
        let messages: Vec<OpenAiMessage> = request
            .messages
            .iter()
            .map(|m| OpenAiMessage {
                role: match m.role {
                    ChatRole::System => "system".to_string(),
                    ChatRole::User => "user".to_string(),
                    ChatRole::Assistant => "assistant".to_string(),
                    ChatRole::Tool => "tool".to_string(),
                },
                content: m.content.clone(),
            })
            .collect();

        let tools = if request.tools.is_empty() {
            None
        } else {
            Some(
                request
                    .tools
                    .iter()
                    .map(|t| OpenAiTool {
                        r#type: "function".to_string(),
                        function: OpenAiFunction {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            parameters: t.parameters.clone(),
                        },
                    })
                    .collect(),
            )
        };

        OpenAiRequest {
            model: self.model.clone(),
            messages,
            tools,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            stream: request.stream,
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let start = std::time::Instant::now();
        let body = self.build_request_body(&CompletionRequest {
            stream: false,
            ..request.clone()
        });

        debug!(provider = %self.name, model = %self.model, "Sending completion request");

        let mut req_builder = self.client.post(self.endpoint()).json(&body);

        if let Some(ref key) = self.api_key {
            req_builder = req_builder.bearer_auth(key);
        }

        let response = req_builder.send().await.map_err(|e| {
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

        let oai_response: OpenAiResponse =
            response
                .json()
                .await
                .map_err(|e| LlmError::InvalidResponse {
                    provider: self.name.clone(),
                    reason: format!("Failed to parse response: {e}"),
                })?;

        let choice =
            oai_response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| LlmError::InvalidResponse {
                    provider: self.name.clone(),
                    reason: "No choices in response".to_string(),
                })?;

        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| LlmToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: tc.function.arguments,
            })
            .collect();

        Ok(CompletionResponse {
            content: choice.message.content.unwrap_or_default(),
            tool_calls,
            usage: TokenUsage {
                prompt_tokens: oai_response.usage.prompt_tokens,
                completion_tokens: oai_response.usage.completion_tokens,
                total_tokens: oai_response.usage.total_tokens,
            },
            model: oai_response.model,
        })
    }

    async fn complete_stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'static, Result<CompletionChunk, LlmError>>, LlmError> {
        let start = std::time::Instant::now();
        let body = self.build_request_body(&CompletionRequest {
            stream: true,
            ..request.clone()
        });

        debug!(provider = %self.name, model = %self.model, "Sending streaming completion request");

        let mut req_builder = self.client.post(self.endpoint()).json(&body);

        if let Some(ref key) = self.api_key {
            req_builder = req_builder.bearer_auth(key);
        }

        let response = req_builder.send().await.map_err(|e| {
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
                    parse_sse_chunks(&text, &provider_name)
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

/// Parse Server-Sent Events (SSE) data lines into completion chunks.
fn parse_sse_chunks(text: &str, provider_name: &str) -> Vec<Result<CompletionChunk, LlmError>> {
    let mut results = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                results.push(Ok(CompletionChunk {
                    delta_content: None,
                    delta_tool_calls: vec![],
                    done: true,
                    usage: None,
                }));
                continue;
            }

            match serde_json::from_str::<OpenAiStreamChunk>(data) {
                Ok(chunk) => {
                    if let Some(choice) = chunk.choices.into_iter().next() {
                        let tool_calls = choice
                            .delta
                            .tool_calls
                            .unwrap_or_default()
                            .into_iter()
                            .map(|tc| LlmToolCall {
                                id: tc.id.unwrap_or_default(),
                                name: tc
                                    .function
                                    .map(|f| f.name.unwrap_or_default())
                                    .unwrap_or_default(),
                                arguments: tc.function_args_fragment.unwrap_or_default(),
                            })
                            .collect();

                        results.push(Ok(CompletionChunk {
                            delta_content: choice.delta.content,
                            delta_tool_calls: tool_calls,
                            done: choice.finish_reason.is_some(),
                            usage: chunk.usage.map(|u| TokenUsage {
                                prompt_tokens: u.prompt_tokens,
                                completion_tokens: u.completion_tokens,
                                total_tokens: u.total_tokens,
                            }),
                        }));
                    }
                }
                Err(e) => {
                    warn!(provider = %provider_name, "Failed to parse SSE chunk: {e}");
                }
            }
        }
    }

    results
}

// =============================================================================
// OpenAI API wire types (internal)
// =============================================================================

#[derive(Debug, Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct OpenAiTool {
    r#type: String,
    function: OpenAiFunction,
}

#[derive(Debug, Serialize)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    model: String,
    choices: Vec<OpenAiChoice>,
    usage: OpenAiUsage,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

// Streaming types
#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiStreamChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    delta: OpenAiStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCall {
    id: Option<String>,
    function: Option<OpenAiStreamToolCallFunction>,
    /// Fragment of arguments JSON for streaming tool calls.
    #[serde(rename = "function")]
    function_args_fragment: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCallFunction {
    name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeout_clamping() {
        let provider = OpenAiProvider::new("test", "http://localhost", None, "gpt-4", 3);
        assert_eq!(provider.timeout, Duration::from_secs(5));

        let provider = OpenAiProvider::new("test", "http://localhost", None, "gpt-4", 200);
        assert_eq!(provider.timeout, Duration::from_secs(120));

        let provider = OpenAiProvider::new("test", "http://localhost", None, "gpt-4", 60);
        assert_eq!(provider.timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_endpoint_construction() {
        let provider = OpenAiProvider::new("test", "https://api.openai.com", None, "gpt-4", 30);
        assert_eq!(
            provider.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );

        // Trailing slash should be stripped
        let provider = OpenAiProvider::new("test", "https://api.openai.com/", None, "gpt-4", 30);
        assert_eq!(
            provider.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_request_body_construction() {
        let provider = OpenAiProvider::new("test", "http://localhost", None, "gpt-4o", 30);

        let request = CompletionRequest {
            messages: vec![
                ChatMessage {
                    role: ChatRole::System,
                    content: "You are helpful.".to_string(),
                },
                ChatMessage {
                    role: ChatRole::User,
                    content: "Hello".to_string(),
                },
            ],
            tools: vec![],
            max_tokens: Some(1000),
            temperature: Some(0.7),
            stream: false,
        };

        let body = provider.build_request_body(&request);
        assert_eq!(body.model, "gpt-4o");
        assert_eq!(body.messages.len(), 2);
        assert_eq!(body.messages[0].role, "system");
        assert_eq!(body.messages[1].role, "user");
        assert!(body.tools.is_none());
        assert_eq!(body.max_tokens, Some(1000));
        assert_eq!(body.temperature, Some(0.7));
        assert!(!body.stream);
    }

    #[test]
    fn test_request_body_with_tools() {
        let provider = OpenAiProvider::new("test", "http://localhost", None, "gpt-4o", 30);

        let request = CompletionRequest {
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: "Search for cats".to_string(),
            }],
            tools: vec![ToolDefinition {
                name: "search".to_string(),
                description: "Search the web".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
            }],
            max_tokens: None,
            temperature: None,
            stream: false,
        };

        let body = provider.build_request_body(&request);
        assert!(body.tools.is_some());
        let tools = body.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function.name, "search");
    }

    #[test]
    fn test_parse_sse_done() {
        let text = "data: [DONE]\n\n";
        let results = parse_sse_chunks(text, "test");
        assert_eq!(results.len(), 1);
        let chunk = results[0].as_ref().unwrap();
        assert!(chunk.done);
        assert!(chunk.delta_content.is_none());
    }

    #[test]
    fn test_parse_sse_content_chunk() {
        let text = r#"data: {"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let results = parse_sse_chunks(text, "test");
        assert_eq!(results.len(), 1);
        let chunk = results[0].as_ref().unwrap();
        assert_eq!(chunk.delta_content.as_deref(), Some("Hello"));
        assert!(!chunk.done);
    }

    #[test]
    fn test_parse_sse_ignores_comments_and_empty() {
        let text = ": this is a comment\n\n\ndata: [DONE]\n";
        let results = parse_sse_chunks(text, "test");
        assert_eq!(results.len(), 1);
    }
}
