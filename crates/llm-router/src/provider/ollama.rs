//! Ollama local inference client.
//!
//! Implements the Ollama chat API (POST /api/chat).

use std::time::Duration;

use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::LlmProvider;
use crate::types::*;
use common::config::ProviderConfig;
use common::LlmError;

/// Client for the Ollama local inference API.
pub struct OllamaProvider {
    name: String,
    client: Client,
    base_url: String,
    model: String,
    timeout: Duration,
}

impl OllamaProvider {
    /// Create a new Ollama provider from a `ProviderConfig`.
    pub fn from_config(config: &ProviderConfig) -> Self {
        Self::new(
            &config.name,
            &config.base_url,
            &config.model,
            config.timeout_seconds,
        )
    }

    /// Create a new Ollama provider client.
    ///
    /// # Arguments
    /// * `name` - Display name for this provider
    /// * `base_url` - Base URL (e.g., "http://localhost:11434")
    /// * `model` - Model identifier (e.g., "llama3.1")
    /// * `timeout_seconds` - Request timeout (clamped to 5–120s, default 30s)
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
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
            model: model.into(),
            timeout,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/api/chat", self.base_url)
    }

    fn build_request_body(&self, request: &CompletionRequest, stream: bool) -> OllamaRequest {
        let messages: Vec<OllamaMessage> = request
            .messages
            .iter()
            .map(|m| OllamaMessage {
                role: match m.role {
                    ChatRole::System => "system".to_string(),
                    ChatRole::User => "user".to_string(),
                    ChatRole::Assistant => "assistant".to_string(),
                    ChatRole::Tool => "tool".to_string(),
                },
                content: m.content.clone(),
                tool_calls: None,
            })
            .collect();

        let tools = if request.tools.is_empty() {
            None
        } else {
            Some(
                request
                    .tools
                    .iter()
                    .map(|t| OllamaTool {
                        r#type: "function".to_string(),
                        function: OllamaFunction {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            parameters: t.parameters.clone(),
                        },
                    })
                    .collect(),
            )
        };

        let options = OllamaOptions {
            temperature: request.temperature,
            num_predict: request.max_tokens.map(|t| t as i64),
        };

        OllamaRequest {
            model: self.model.clone(),
            messages,
            tools,
            stream,
            options: Some(options),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let start = std::time::Instant::now();
        let body = self.build_request_body(request, false);

        debug!(provider = %self.name, model = %self.model, "Sending Ollama completion request");

        let response = self
            .client
            .post(self.endpoint())
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

        let ollama_response: OllamaResponse =
            response.json().await.map_err(|e| LlmError::InvalidResponse {
                provider: self.name.clone(),
                reason: format!("Failed to parse response: {e}"),
            })?;

        let tool_calls = ollama_response
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| LlmToolCall {
                id: uuid::Uuid::new_v4().to_string(),
                name: tc.function.name,
                arguments: serde_json::to_string(&tc.function.arguments).unwrap_or_default(),
            })
            .collect();

        // Ollama doesn't provide token counts in the same way; estimate from response
        let prompt_tokens = ollama_response.prompt_eval_count.unwrap_or(0);
        let completion_tokens = ollama_response.eval_count.unwrap_or(0);

        Ok(CompletionResponse {
            content: ollama_response.message.content,
            tool_calls,
            usage: TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            },
            model: ollama_response.model,
        })
    }

    async fn complete_stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'static, Result<CompletionChunk, LlmError>>, LlmError> {
        let start = std::time::Instant::now();
        let body = self.build_request_body(request, true);

        debug!(provider = %self.name, model = %self.model, "Sending Ollama streaming request");

        let response = self
            .client
            .post(self.endpoint())
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

        // Ollama streams newline-delimited JSON (NDJSON)
        let stream = byte_stream
            .map(move |chunk_result| match chunk_result {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    parse_ollama_ndjson(&text, &provider_name)
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

/// Parse Ollama NDJSON streaming responses into completion chunks.
fn parse_ollama_ndjson(text: &str, provider_name: &str) -> Vec<Result<CompletionChunk, LlmError>> {
    let mut results = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match serde_json::from_str::<OllamaStreamChunk>(line) {
            Ok(chunk) => {
                let tool_calls = chunk
                    .message
                    .tool_calls
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tc| LlmToolCall {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: tc.function.name,
                        arguments: serde_json::to_string(&tc.function.arguments)
                            .unwrap_or_default(),
                    })
                    .collect();

                let usage = if chunk.done {
                    Some(TokenUsage {
                        prompt_tokens: chunk.prompt_eval_count.unwrap_or(0),
                        completion_tokens: chunk.eval_count.unwrap_or(0),
                        total_tokens: chunk.prompt_eval_count.unwrap_or(0)
                            + chunk.eval_count.unwrap_or(0),
                    })
                } else {
                    None
                };

                results.push(Ok(CompletionChunk {
                    delta_content: if chunk.message.content.is_empty() {
                        None
                    } else {
                        Some(chunk.message.content)
                    },
                    delta_tool_calls: tool_calls,
                    done: chunk.done,
                    usage,
                }));
            }
            Err(e) => {
                tracing::warn!(provider = %provider_name, "Failed to parse Ollama NDJSON: {e}");
            }
        }
    }

    results
}

// =============================================================================
// Ollama API wire types (internal)
// =============================================================================

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Debug, Serialize)]
struct OllamaTool {
    r#type: String,
    function: OllamaFunction,
}

#[derive(Debug, Serialize)]
struct OllamaFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    model: String,
    message: OllamaMessage,
    prompt_eval_count: Option<u32>,
    eval_count: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolCall {
    function: OllamaToolCallFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolCallFunction {
    name: String,
    arguments: serde_json::Value,
}

// Streaming types
#[derive(Debug, Deserialize)]
struct OllamaStreamChunk {
    message: OllamaMessage,
    done: bool,
    prompt_eval_count: Option<u32>,
    eval_count: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeout_clamping() {
        let provider = OllamaProvider::new("test", "http://localhost:11434", "llama3.1", 3);
        assert_eq!(provider.timeout, Duration::from_secs(5));

        let provider = OllamaProvider::new("test", "http://localhost:11434", "llama3.1", 200);
        assert_eq!(provider.timeout, Duration::from_secs(120));

        let provider = OllamaProvider::new("test", "http://localhost:11434", "llama3.1", 45);
        assert_eq!(provider.timeout, Duration::from_secs(45));
    }

    #[test]
    fn test_endpoint_construction() {
        let provider = OllamaProvider::new("test", "http://localhost:11434", "llama3.1", 30);
        assert_eq!(provider.endpoint(), "http://localhost:11434/api/chat");

        let provider = OllamaProvider::new("test", "http://localhost:11434/", "llama3.1", 30);
        assert_eq!(provider.endpoint(), "http://localhost:11434/api/chat");
    }

    #[test]
    fn test_request_body_construction() {
        let provider = OllamaProvider::new("test", "http://localhost:11434", "llama3.1", 30);

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
            max_tokens: Some(500),
            temperature: Some(0.8),
            stream: false,
        };

        let body = provider.build_request_body(&request, false);
        assert_eq!(body.model, "llama3.1");
        assert_eq!(body.messages.len(), 2);
        assert_eq!(body.messages[0].role, "system");
        assert!(!body.stream);
        assert!(body.tools.is_none());
        let opts = body.options.unwrap();
        assert_eq!(opts.temperature, Some(0.8));
        assert_eq!(opts.num_predict, Some(500));
    }

    #[test]
    fn test_parse_ollama_ndjson_content() {
        let text = r#"{"model":"llama3.1","message":{"role":"assistant","content":"Hi"},"done":false}"#;
        let results = parse_ollama_ndjson(text, "test");
        assert_eq!(results.len(), 1);
        let chunk = results[0].as_ref().unwrap();
        assert_eq!(chunk.delta_content.as_deref(), Some("Hi"));
        assert!(!chunk.done);
    }

    #[test]
    fn test_parse_ollama_ndjson_done() {
        let text = r#"{"model":"llama3.1","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":10,"eval_count":20}"#;
        let results = parse_ollama_ndjson(text, "test");
        assert_eq!(results.len(), 1);
        let chunk = results[0].as_ref().unwrap();
        assert!(chunk.done);
        let usage = chunk.usage.as_ref().unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 20);
        assert_eq!(usage.total_tokens, 30);
    }

    #[test]
    fn test_parse_ollama_ndjson_multiple_lines() {
        let text = r#"{"model":"llama3.1","message":{"role":"assistant","content":"Hello"},"done":false}
{"model":"llama3.1","message":{"role":"assistant","content":" world"},"done":false}
{"model":"llama3.1","message":{"role":"assistant","content":""},"done":true,"eval_count":5}"#;
        let results = parse_ollama_ndjson(text, "test");
        assert_eq!(results.len(), 3);
        assert_eq!(
            results[0].as_ref().unwrap().delta_content.as_deref(),
            Some("Hello")
        );
        assert_eq!(
            results[1].as_ref().unwrap().delta_content.as_deref(),
            Some(" world")
        );
        assert!(results[2].as_ref().unwrap().done);
    }
}
