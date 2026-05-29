//! LLM provider trait and implementations.

mod anthropic;
mod cli_provider;
mod ollama;
mod openai;

pub use anthropic::AnthropicProvider;
pub use cli_provider::{
    ClaudeCodeProvider, CodexCliProvider, CopilotCliProvider, GeminiCliProvider,
};
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;

use crate::types::{CompletionChunk, CompletionRequest, CompletionResponse};
use common::config::{ProviderConfig, ProviderType};
use common::LlmError;
use futures::stream::BoxStream;

/// Trait for LLM provider clients.
///
/// Each provider implements async completion (both streaming and non-streaming)
/// with configurable timeouts.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// The provider's display name (used in logging and error messages).
    fn name(&self) -> &str;

    /// Whether the configured model can accept image input (vision). Defaults
    /// to false; providers override based on the model identifier.
    fn supports_vision(&self) -> bool {
        false
    }

    /// Send a completion request and return the full response.
    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, LlmError>;

    /// Send a completion request and return a stream of response chunks.
    async fn complete_stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'static, Result<CompletionChunk, LlmError>>, LlmError>;
}

/// Create an LLM provider client from a `ProviderConfig`.
///
/// Dispatches to the appropriate provider implementation based on `provider_type`.
pub fn create_provider(config: &ProviderConfig) -> Box<dyn LlmProvider> {
    match config.provider_type {
        ProviderType::OpenAiCompatible => Box::new(OpenAiProvider::from_config(config)),
        ProviderType::Anthropic => Box::new(AnthropicProvider::from_config(config)),
        ProviderType::Ollama => Box::new(OllamaProvider::from_config(config)),
        ProviderType::ClaudeCode => Box::new(ClaudeCodeProvider::from_config(config)),
        ProviderType::CopilotCli => Box::new(CopilotCliProvider::from_config(config)),
        ProviderType::GeminiCli => Box::new(GeminiCliProvider::from_config(config)),
        ProviderType::CodexCli => Box::new(CodexCliProvider::from_config(config)),
    }
}
