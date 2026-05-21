//! LLM Router — routes LLM requests to configured providers with failover support.
//!
//! Responsibilities:
//! - Maintain provider configurations and health status
//! - Route requests based on priority ordering
//! - Handle timeouts and failover logic
//! - Log all routing decisions and failures

pub mod provider;
pub mod router;
pub mod types;

pub use provider::{
    create_provider, AnthropicProvider, ClaudeCodeProvider, CopilotCliProvider, LlmProvider,
    OllamaProvider, OpenAiProvider,
};
pub use router::{LlmRouter, ProviderHealth, ProviderStatus};
pub use types::*;
