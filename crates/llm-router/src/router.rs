//! Failover routing logic for LLM providers.
//!
//! Routes requests to providers in strict priority order. On failure or timeout,
//! automatically fails over to the next provider. Never retries the same provider
//! within a single request. Logs all failover events.

use futures::stream::BoxStream;
use tracing::{error, info, warn};

use crate::provider::{create_provider, LlmProvider};
use crate::types::{CompletionChunk, CompletionRequest, CompletionResponse};
use common::config::{LlmConfig, ProviderConfig};
use common::errors::{LlmError, ProviderAttempt};

/// Health status of a single provider.
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    /// Display name of the provider.
    pub name: String,
    /// Priority value (lower = higher priority).
    pub priority: u8,
    /// Current status.
    pub status: ProviderStatus,
    /// Last observed latency in milliseconds.
    pub last_latency_ms: Option<u64>,
}

/// Status of a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStatus {
    /// Provider is available and responding.
    Healthy,
    /// Provider is experiencing issues but may still work.
    Degraded { reason: String },
    /// Provider is not responding.
    Unavailable { reason: String },
}

/// A provider entry sorted by priority, holding the client and config metadata.
struct ProviderEntry {
    /// The provider client.
    client: Box<dyn LlmProvider>,
    /// The provider's configured priority (lower = higher priority).
    priority: u8,
    /// The provider's display name.
    name: String,
}

/// LLM Router that routes requests to providers with automatic failover.
///
/// Providers are tried in strict priority order (lowest priority value first).
/// On failure or timeout, the router moves to the next provider without retrying
/// the failed one. All failover events are logged via `tracing`.
pub struct LlmRouter {
    /// Providers sorted by priority (ascending — lowest priority value first).
    providers: Vec<ProviderEntry>,
}

impl LlmRouter {
    /// Create a new `LlmRouter` from an `LlmConfig`.
    ///
    /// Providers are sorted by priority (lower value = higher priority).
    /// Supports 1–10 providers.
    pub fn from_config(config: &LlmConfig) -> Self {
        let mut entries: Vec<ProviderEntry> = config
            .providers
            .iter()
            .map(|pc| {
                let client = create_provider(pc);
                ProviderEntry {
                    client,
                    priority: pc.priority,
                    name: pc.name.clone(),
                }
            })
            .collect();

        // Sort by priority ascending (lower value = higher priority = tried first).
        entries.sort_by_key(|e| e.priority);

        info!(
            provider_count = entries.len(),
            order = ?entries.iter().map(|e| format!("{}(p{})", e.name, e.priority)).collect::<Vec<_>>(),
            "LLM Router initialized with providers in priority order"
        );

        Self { providers: entries }
    }

    /// Create a new `LlmRouter` from a list of `ProviderConfig`s.
    pub fn from_provider_configs(configs: &[ProviderConfig]) -> Self {
        let llm_config = LlmConfig {
            providers: configs.to_vec(),
        };
        Self::from_config(&llm_config)
    }

    /// Send a completion request, trying providers in priority order with failover.
    ///
    /// On failure/timeout, routes to the next provider. Never retries the same
    /// provider within a single request. Returns `LlmError::AllProvidersFailed`
    /// if all providers fail.
    pub async fn complete(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, LlmError> {
        if self.providers.is_empty() {
            return Err(LlmError::AllProvidersFailed { attempts: vec![] });
        }

        let mut attempts: Vec<ProviderAttempt> = Vec::new();

        for (idx, entry) in self.providers.iter().enumerate() {
            let start = std::time::Instant::now();

            match entry.client.complete(request).await {
                Ok(response) => {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    if !attempts.is_empty() {
                        // We had previous failures — log successful failover
                        info!(
                            provider = %entry.name,
                            priority = entry.priority,
                            elapsed_ms,
                            previous_failures = attempts.len(),
                            "Request succeeded after failover"
                        );
                    }
                    return Ok(response);
                }
                Err(err) => {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    let error_str = err.to_string();

                    // Log the failover event
                    let next_provider = self.providers.get(idx + 1).map(|e| e.name.as_str());

                    if let Some(fallback) = next_provider {
                        warn!(
                            original_provider = %entry.name,
                            failure_reason = %error_str,
                            fallback_target = %fallback,
                            elapsed_ms,
                            "Provider failed, routing to next provider"
                        );
                    } else {
                        error!(
                            provider = %entry.name,
                            failure_reason = %error_str,
                            elapsed_ms,
                            "Last provider failed, no more providers available"
                        );
                    }

                    attempts.push(ProviderAttempt {
                        provider: entry.name.clone(),
                        error: error_str,
                        elapsed_ms,
                    });
                }
            }
        }

        Err(LlmError::AllProvidersFailed { attempts })
    }

    /// Send a streaming completion request, trying providers in priority order with failover.
    ///
    /// On failure/timeout during stream initiation, routes to the next provider.
    /// Never retries the same provider within a single request.
    pub async fn complete_stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'static, Result<CompletionChunk, LlmError>>, LlmError> {
        if self.providers.is_empty() {
            return Err(LlmError::AllProvidersFailed { attempts: vec![] });
        }

        let mut attempts: Vec<ProviderAttempt> = Vec::new();

        for (idx, entry) in self.providers.iter().enumerate() {
            let start = std::time::Instant::now();

            match entry.client.complete_stream(request).await {
                Ok(stream) => {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    if !attempts.is_empty() {
                        info!(
                            provider = %entry.name,
                            priority = entry.priority,
                            elapsed_ms,
                            previous_failures = attempts.len(),
                            "Streaming request succeeded after failover"
                        );
                    }
                    return Ok(stream);
                }
                Err(err) => {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    let error_str = err.to_string();

                    let next_provider = self.providers.get(idx + 1).map(|e| e.name.as_str());

                    if let Some(fallback) = next_provider {
                        warn!(
                            original_provider = %entry.name,
                            failure_reason = %error_str,
                            fallback_target = %fallback,
                            elapsed_ms,
                            "Streaming provider failed, routing to next provider"
                        );
                    } else {
                        error!(
                            provider = %entry.name,
                            failure_reason = %error_str,
                            elapsed_ms,
                            "Last streaming provider failed, no more providers available"
                        );
                    }

                    attempts.push(ProviderAttempt {
                        provider: entry.name.clone(),
                        error: error_str,
                        elapsed_ms,
                    });
                }
            }
        }

        Err(LlmError::AllProvidersFailed { attempts })
    }

    /// Get health status of all configured providers.
    pub fn provider_status(&self) -> Vec<ProviderHealth> {
        self.providers
            .iter()
            .map(|entry| ProviderHealth {
                name: entry.name.clone(),
                priority: entry.priority,
                status: ProviderStatus::Healthy,
                last_latency_ms: None,
            })
            .collect()
    }

    /// Reload the router with a new configuration.
    ///
    /// Replaces all provider clients with new ones from the given config.
    pub fn reload_config(&mut self, config: &LlmConfig) {
        let mut entries: Vec<ProviderEntry> = config
            .providers
            .iter()
            .map(|pc| {
                let client = create_provider(pc);
                ProviderEntry {
                    client,
                    priority: pc.priority,
                    name: pc.name.clone(),
                }
            })
            .collect();

        entries.sort_by_key(|e| e.priority);

        info!(
            provider_count = entries.len(),
            order = ?entries.iter().map(|e| format!("{}(p{})", e.name, e.priority)).collect::<Vec<_>>(),
            "LLM Router reloaded with new configuration"
        );

        self.providers = entries;
    }

    /// Returns the number of configured providers.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use common::config::{ProviderConfig, ProviderType};
    use futures::stream::BoxStream;
    use proptest::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // =========================================================================
    // Mock provider for testing
    // =========================================================================

    /// A mock provider that can be configured to succeed or fail.
    struct MockProvider {
        name: String,
        /// If Some, the provider will return this error.
        fail_with: Option<LlmError>,
        /// Tracks how many times complete() was called.
        call_count: Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn succeeding(name: &str, call_count: Arc<AtomicUsize>) -> Self {
            Self {
                name: name.to_string(),
                fail_with: None,
                call_count,
            }
        }

        fn failing(name: &str, error: LlmError, call_count: Arc<AtomicUsize>) -> Self {
            Self {
                name: name.to_string(),
                fail_with: Some(error),
                call_count,
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            &self.name
        }

        async fn complete(
            &self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if let Some(ref err) = self.fail_with {
                // Clone the error by recreating it
                match err {
                    LlmError::Timeout {
                        provider,
                        elapsed_ms,
                    } => Err(LlmError::Timeout {
                        provider: provider.clone(),
                        elapsed_ms: *elapsed_ms,
                    }),
                    LlmError::InvalidResponse { provider, reason } => {
                        Err(LlmError::InvalidResponse {
                            provider: provider.clone(),
                            reason: reason.clone(),
                        })
                    }
                    LlmError::RateLimited {
                        provider,
                        retry_after,
                    } => Err(LlmError::RateLimited {
                        provider: provider.clone(),
                        retry_after: *retry_after,
                    }),
                    LlmError::AllProvidersFailed { attempts } => {
                        Err(LlmError::AllProvidersFailed {
                            attempts: attempts.clone(),
                        })
                    }
                }
            } else {
                Ok(CompletionResponse {
                    content: format!("Response from {}", self.name),
                    tool_calls: vec![],
                    usage: TokenUsage::default(),
                    model: "mock-model".to_string(),
                })
            }
        }

        async fn complete_stream(
            &self,
            _request: &CompletionRequest,
        ) -> Result<BoxStream<'static, Result<CompletionChunk, LlmError>>, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if let Some(ref err) = self.fail_with {
                match err {
                    LlmError::Timeout {
                        provider,
                        elapsed_ms,
                    } => Err(LlmError::Timeout {
                        provider: provider.clone(),
                        elapsed_ms: *elapsed_ms,
                    }),
                    LlmError::InvalidResponse { provider, reason } => {
                        Err(LlmError::InvalidResponse {
                            provider: provider.clone(),
                            reason: reason.clone(),
                        })
                    }
                    LlmError::RateLimited {
                        provider,
                        retry_after,
                    } => Err(LlmError::RateLimited {
                        provider: provider.clone(),
                        retry_after: *retry_after,
                    }),
                    LlmError::AllProvidersFailed { attempts } => {
                        Err(LlmError::AllProvidersFailed {
                            attempts: attempts.clone(),
                        })
                    }
                }
            } else {
                let chunk = CompletionChunk {
                    delta_content: Some(format!("Stream from {}", self.name)),
                    delta_tool_calls: vec![],
                    done: true,
                    usage: Some(TokenUsage::default()),
                };
                Ok(Box::pin(futures::stream::once(async move { Ok(chunk) })))
            }
        }
    }

    // =========================================================================
    // Helper to build a router with mock providers
    // =========================================================================

    fn build_test_router(providers: Vec<ProviderEntry>) -> LlmRouter {
        LlmRouter { providers }
    }

    fn make_request() -> CompletionRequest {
        CompletionRequest {
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: "Hello".to_string(),
            }],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            stream: false,
        }
    }

    // =========================================================================
    // Tests
    // =========================================================================

    #[tokio::test]
    async fn test_single_provider_success() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let router = build_test_router(vec![ProviderEntry {
            client: Box::new(MockProvider::succeeding("primary", call_count.clone())),
            priority: 1,
            name: "primary".to_string(),
        }]);

        let result = router.complete(&make_request()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "Response from primary");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_failover_to_second_provider() {
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));

        let router = build_test_router(vec![
            ProviderEntry {
                client: Box::new(MockProvider::failing(
                    "primary",
                    LlmError::Timeout {
                        provider: "primary".to_string(),
                        elapsed_ms: 30000,
                    },
                    count_a.clone(),
                )),
                priority: 1,
                name: "primary".to_string(),
            },
            ProviderEntry {
                client: Box::new(MockProvider::succeeding("secondary", count_b.clone())),
                priority: 2,
                name: "secondary".to_string(),
            },
        ]);

        let result = router.complete(&make_request()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "Response from secondary");
        // Primary was called once, then failover to secondary
        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_all_providers_fail() {
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));
        let count_c = Arc::new(AtomicUsize::new(0));

        let router = build_test_router(vec![
            ProviderEntry {
                client: Box::new(MockProvider::failing(
                    "provider-a",
                    LlmError::Timeout {
                        provider: "provider-a".to_string(),
                        elapsed_ms: 30000,
                    },
                    count_a.clone(),
                )),
                priority: 1,
                name: "provider-a".to_string(),
            },
            ProviderEntry {
                client: Box::new(MockProvider::failing(
                    "provider-b",
                    LlmError::RateLimited {
                        provider: "provider-b".to_string(),
                        retry_after: Duration::from_secs(60),
                    },
                    count_b.clone(),
                )),
                priority: 2,
                name: "provider-b".to_string(),
            },
            ProviderEntry {
                client: Box::new(MockProvider::failing(
                    "provider-c",
                    LlmError::InvalidResponse {
                        provider: "provider-c".to_string(),
                        reason: "server error".to_string(),
                    },
                    count_c.clone(),
                )),
                priority: 3,
                name: "provider-c".to_string(),
            },
        ]);

        let result = router.complete(&make_request()).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            LlmError::AllProvidersFailed { attempts } => {
                assert_eq!(attempts.len(), 3);
                assert_eq!(attempts[0].provider, "provider-a");
                assert_eq!(attempts[1].provider, "provider-b");
                assert_eq!(attempts[2].provider, "provider-c");
            }
            other => panic!("Expected AllProvidersFailed, got: {other:?}"),
        }

        // Each provider called exactly once (no retries)
        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 1);
        assert_eq!(count_c.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_no_providers_returns_error() {
        let router = build_test_router(vec![]);

        let result = router.complete(&make_request()).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            LlmError::AllProvidersFailed { attempts } => {
                assert!(attempts.is_empty());
            }
            other => panic!("Expected AllProvidersFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        // Providers added out of order — router should try them in priority order
        let count_low = Arc::new(AtomicUsize::new(0));
        let count_high = Arc::new(AtomicUsize::new(0));

        let configs = vec![
            ProviderConfig {
                name: "low-priority".to_string(),
                provider_type: ProviderType::Ollama,
                api_key: None,
                base_url: "http://localhost:11434".to_string(),
                model: "llama2".to_string(),
                priority: 10,
                timeout_seconds: 30,
                max_tokens: None,
            },
            ProviderConfig {
                name: "high-priority".to_string(),
                provider_type: ProviderType::Ollama,
                api_key: None,
                base_url: "http://localhost:11434".to_string(),
                model: "llama2".to_string(),
                priority: 1,
                timeout_seconds: 30,
                max_tokens: None,
            },
        ];

        let router = LlmRouter::from_provider_configs(&configs);

        // Verify internal ordering
        assert_eq!(router.providers[0].name, "high-priority");
        assert_eq!(router.providers[1].name, "low-priority");
    }

    #[tokio::test]
    async fn test_failover_stream_to_second_provider() {
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));

        let router = build_test_router(vec![
            ProviderEntry {
                client: Box::new(MockProvider::failing(
                    "primary",
                    LlmError::Timeout {
                        provider: "primary".to_string(),
                        elapsed_ms: 30000,
                    },
                    count_a.clone(),
                )),
                priority: 1,
                name: "primary".to_string(),
            },
            ProviderEntry {
                client: Box::new(MockProvider::succeeding("secondary", count_b.clone())),
                priority: 2,
                name: "secondary".to_string(),
            },
        ]);

        let result = router.complete_stream(&make_request()).await;
        assert!(result.is_ok());
        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_provider_status_returns_all_providers() {
        let count = Arc::new(AtomicUsize::new(0));
        let router = build_test_router(vec![
            ProviderEntry {
                client: Box::new(MockProvider::succeeding("alpha", count.clone())),
                priority: 1,
                name: "alpha".to_string(),
            },
            ProviderEntry {
                client: Box::new(MockProvider::succeeding("beta", count.clone())),
                priority: 2,
                name: "beta".to_string(),
            },
        ]);

        let status = router.provider_status();
        assert_eq!(status.len(), 2);
        assert_eq!(status[0].name, "alpha");
        assert_eq!(status[0].priority, 1);
        assert_eq!(status[1].name, "beta");
        assert_eq!(status[1].priority, 2);
    }

    #[tokio::test]
    async fn test_reload_config_replaces_providers() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut router = build_test_router(vec![ProviderEntry {
            client: Box::new(MockProvider::succeeding("old", count.clone())),
            priority: 1,
            name: "old".to_string(),
        }]);

        assert_eq!(router.provider_count(), 1);

        let new_config = LlmConfig {
            providers: vec![
                ProviderConfig {
                    name: "new-a".to_string(),
                    provider_type: ProviderType::Ollama,
                    api_key: None,
                    base_url: "http://localhost:11434".to_string(),
                    model: "llama2".to_string(),
                    priority: 1,
                    timeout_seconds: 30,
                    max_tokens: None,
                },
                ProviderConfig {
                    name: "new-b".to_string(),
                    provider_type: ProviderType::Ollama,
                    api_key: None,
                    base_url: "http://localhost:11434".to_string(),
                    model: "llama2".to_string(),
                    priority: 2,
                    timeout_seconds: 30,
                    max_tokens: None,
                },
            ],
        };

        router.reload_config(&new_config);
        assert_eq!(router.provider_count(), 2);

        let status = router.provider_status();
        assert_eq!(status[0].name, "new-a");
        assert_eq!(status[1].name, "new-b");
    }

    #[tokio::test]
    async fn test_never_retries_same_provider() {
        // With 3 providers where first two fail, ensure each is called exactly once
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));
        let count_c = Arc::new(AtomicUsize::new(0));

        let router = build_test_router(vec![
            ProviderEntry {
                client: Box::new(MockProvider::failing(
                    "a",
                    LlmError::Timeout {
                        provider: "a".to_string(),
                        elapsed_ms: 5000,
                    },
                    count_a.clone(),
                )),
                priority: 1,
                name: "a".to_string(),
            },
            ProviderEntry {
                client: Box::new(MockProvider::failing(
                    "b",
                    LlmError::InvalidResponse {
                        provider: "b".to_string(),
                        reason: "500 error".to_string(),
                    },
                    count_b.clone(),
                )),
                priority: 2,
                name: "b".to_string(),
            },
            ProviderEntry {
                client: Box::new(MockProvider::succeeding("c", count_c.clone())),
                priority: 3,
                name: "c".to_string(),
            },
        ]);

        let result = router.complete(&make_request()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "Response from c");

        // Each provider called exactly once — no retries
        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 1);
        assert_eq!(count_c.load(Ordering::SeqCst), 1);
    }

    // =========================================================================
    // Property-Based Tests
    //
    // Property 1: LLM Router Failover Correctness
    // Property 2: LLM Provider Configuration Validation
    //
    // **Validates: Requirements 1.4, 1.5, 1.6**
    // =========================================================================

    /// Strategy: generate a vector of 1–10 provider specs with distinct priorities
    /// and a random failure pattern (each provider either succeeds or fails).
    fn provider_specs_strategy() -> impl Strategy<Value = Vec<(u8, bool)>> {
        // Generate 1-10 distinct priorities, then pair each with a fail/succeed flag
        (1usize..=10).prop_flat_map(|n| {
            // Generate n distinct priorities from 1..=255
            proptest::collection::hash_set(1u8..=255, n).prop_flat_map(move |priorities| {
                let priorities_vec: Vec<u8> = priorities.into_iter().collect();
                // For each priority, generate a boolean indicating failure
                proptest::collection::vec(proptest::bool::ANY, n).prop_map(move |failures| {
                    priorities_vec
                        .iter()
                        .zip(failures.iter())
                        .map(|(&p, &f)| (p, f))
                        .collect::<Vec<(u8, bool)>>()
                })
            })
        })
    }

    /// Helper: build a router from (priority, should_fail) specs and return
    /// the router plus per-provider call counters in priority order.
    fn build_property_test_router(
        specs: &[(u8, bool)],
    ) -> (LlmRouter, Vec<(String, u8, bool, Arc<AtomicUsize>)>) {
        // Sort by priority (ascending) to match router's internal ordering
        let mut indexed_specs: Vec<(usize, u8, bool)> = specs
            .iter()
            .enumerate()
            .map(|(i, &(p, f))| (i, p, f))
            .collect();
        indexed_specs.sort_by_key(|&(_, p, _)| p);

        let mut entries = Vec::new();
        let mut info = Vec::new();

        for (idx, priority, should_fail) in &indexed_specs {
            let name = format!("provider-{}", idx);
            let counter = Arc::new(AtomicUsize::new(0));
            info.push((name.clone(), *priority, *should_fail, counter.clone()));

            let mock: Box<dyn LlmProvider> = if *should_fail {
                Box::new(MockProvider::failing(
                    &name,
                    LlmError::Timeout {
                        provider: name.clone(),
                        elapsed_ms: 30000,
                    },
                    counter,
                ))
            } else {
                Box::new(MockProvider::succeeding(&name, counter))
            };

            entries.push(ProviderEntry {
                client: mock,
                priority: *priority,
                name: name.clone(),
            });
        }

        (LlmRouter { providers: entries }, info)
    }

    // We need a synchronous test runner for proptest that spawns a tokio runtime
    // per test case, since proptest doesn't natively support async.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// **Validates: Requirements 1.4, 1.5**
        ///
        /// Property 1: LLM Router Failover Correctness
        ///
        /// For any set of 1–10 configured LLM providers with distinct priorities,
        /// and any failure scenario where K providers fail (0 ≤ K ≤ N), the LLM
        /// Router SHALL:
        /// - Attempt providers in strict priority order
        /// - Never attempt a provider more than once per request
        /// - Return a successful response from the first healthy provider
        /// - Return AllProvidersFailed error if all fail
        #[test]
        fn prop_failover_correctness(specs in provider_specs_strategy()) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async {
                let (router, info) = build_property_test_router(&specs);
                let result = router.complete(&make_request()).await;

                // info is sorted by priority (ascending)
                // Find the first non-failing provider in priority order
                let first_healthy = info.iter().find(|(_, _, should_fail, _)| !should_fail);

                match first_healthy {
                    Some((name, _priority, _, _counter)) => {
                        // Should succeed with response from first healthy provider
                        let response = result.expect("Should succeed when a healthy provider exists");
                        prop_assert_eq!(
                            response.content,
                            format!("Response from {}", name),
                            "Response should come from first healthy provider in priority order"
                        );
                    }
                    None => {
                        // All providers fail — should return AllProvidersFailed
                        let err = result.expect_err("Should fail when all providers fail");
                        match err {
                            LlmError::AllProvidersFailed { attempts } => {
                                prop_assert_eq!(
                                    attempts.len(),
                                    info.len(),
                                    "Should have one attempt per provider"
                                );
                            }
                            other => {
                                prop_assert!(false, "Expected AllProvidersFailed, got: {:?}", other);
                            }
                        }
                    }
                }

                // Verify no provider is called more than once
                for (name, _priority, _should_fail, counter) in &info {
                    let calls = counter.load(Ordering::SeqCst);
                    prop_assert!(
                        calls <= 1,
                        "Provider '{}' was called {} times, expected at most 1",
                        name,
                        calls
                    );
                }

                // Verify providers are attempted in strict priority order:
                // All providers up to and including the first healthy one should be called.
                // Providers after the first healthy one should NOT be called.
                let mut found_healthy = false;
                for (name, _priority, should_fail, counter) in &info {
                    let calls = counter.load(Ordering::SeqCst);
                    if !found_healthy {
                        // This provider should have been attempted
                        prop_assert_eq!(
                            calls, 1,
                            "Provider '{}' (fail={}) should have been called exactly once before first healthy",
                            name, should_fail
                        );
                        if !should_fail {
                            found_healthy = true;
                        }
                    } else {
                        // This provider should NOT have been attempted (comes after first healthy)
                        prop_assert_eq!(
                            calls, 0,
                            "Provider '{}' should not be called after a healthy provider succeeded",
                            name
                        );
                    }
                }

                Ok(())
            })?;
        }
    }

    // =========================================================================
    // Property 2: LLM Provider Configuration Validation
    //
    // **Validates: Requirements 1.6**
    // =========================================================================

    /// Strategy: generate a valid LLM provider config with 1-10 providers,
    /// each having valid fields and unique priorities.
    fn valid_llm_config_strategy() -> impl Strategy<Value = Vec<ProviderConfig>> {
        (1usize..=10).prop_flat_map(|n| {
            // Generate n distinct priorities
            proptest::collection::hash_set(1u8..=255, n).prop_flat_map(move |priorities| {
                let priorities_vec: Vec<u8> = priorities.into_iter().collect();
                proptest::collection::vec(
                    (
                        "[a-z][a-z0-9-]{2,20}", // name
                        "[a-z][a-z0-9-]{2,20}", // model
                        5u32..=120,             // timeout
                    ),
                    n,
                )
                .prop_map(move |fields| {
                    fields
                        .into_iter()
                        .zip(priorities_vec.iter())
                        .map(|((name, model, timeout), &priority)| ProviderConfig {
                            name,
                            provider_type: ProviderType::Ollama,
                            api_key: None,
                            base_url: "http://localhost:11434".to_string(),
                            model,
                            priority,
                            timeout_seconds: timeout,
                            max_tokens: None,
                        })
                        .collect::<Vec<_>>()
                })
            })
        })
    }

    /// Strategy: generate an invalid provider count (0 or 11-15).
    fn invalid_provider_count_strategy() -> impl Strategy<Value = usize> {
        prop_oneof![Just(0usize), 11usize..=15,]
    }

    /// Helper: create a minimal valid PlatformConfig with given LLM providers.
    fn platform_config_with_providers(
        providers: Vec<ProviderConfig>,
    ) -> common::config::PlatformConfig {
        use common::config::*;
        PlatformConfig {
            general: GeneralConfig::default(),
            llm: LlmConfig { providers },
            security: SecurityConfig::default(),
            coding: None,
            scheduler: SchedulerConfig::default(),
            web: WebConfig::default(),
            api: ApiConfig::default(),
            messaging: MessagingConfig::default(),
            monitoring: MonitoringConfig::default(),
            plugins: PluginConfig::default(),
            mcp: McpConfig::default(),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// **Validates: Requirements 1.6**
        ///
        /// Property 2: LLM Provider Configuration Validation — Valid configs accepted
        ///
        /// For any configuration with 1–10 providers each having valid API keys,
        /// model selections, and unique priority orderings, the Config_Manager
        /// SHALL accept the configuration (no LLM-related validation errors).
        #[test]
        fn prop_valid_config_accepted(providers in valid_llm_config_strategy()) {
            let config = platform_config_with_providers(providers);
            let errors = common::config::validate_config(&config);

            // Filter to only LLM-related errors
            let llm_errors: Vec<_> = errors
                .iter()
                .filter(|e| e.setting.starts_with("llm."))
                .collect();

            prop_assert!(
                llm_errors.is_empty(),
                "Valid LLM config should produce no LLM validation errors, got: {:?}",
                llm_errors
            );
        }

        /// **Validates: Requirements 1.6**
        ///
        /// Property 2: LLM Provider Configuration Validation — Invalid count rejected
        ///
        /// Configurations with 0 or more than 10 providers SHALL be rejected.
        #[test]
        fn prop_invalid_provider_count_rejected(count in invalid_provider_count_strategy()) {
            let providers: Vec<ProviderConfig> = (0..count)
                .map(|i| ProviderConfig {
                    name: format!("provider-{}", i),
                    provider_type: ProviderType::Ollama,
                    api_key: None,
                    base_url: "http://localhost:11434".to_string(),
                    model: "llama3".to_string(),
                    priority: i as u8,
                    timeout_seconds: 30,
                    max_tokens: None,
                })
                .collect();

            let config = platform_config_with_providers(providers);
            let errors = common::config::validate_config(&config);

            let llm_errors: Vec<_> = errors
                .iter()
                .filter(|e| e.setting == "llm.providers")
                .collect();

            prop_assert!(
                !llm_errors.is_empty(),
                "Config with {} providers should produce validation error",
                count
            );
        }

        /// **Validates: Requirements 1.6**
        ///
        /// Property 2: LLM Provider Configuration Validation — Duplicate priorities rejected
        ///
        /// Configurations where two or more providers share the same priority
        /// SHALL be rejected.
        #[test]
        fn prop_duplicate_priorities_rejected(
            n in 2usize..=10,
            dup_priority in 1u8..=255,
        ) {
            // Create n providers where at least two share the same priority
            let providers: Vec<ProviderConfig> = (0..n)
                .map(|i| ProviderConfig {
                    name: format!("provider-{}", i),
                    provider_type: ProviderType::Ollama,
                    api_key: None,
                    base_url: "http://localhost:11434".to_string(),
                    model: "llama3".to_string(),
                    priority: dup_priority, // all same priority
                    timeout_seconds: 30,
                    max_tokens: None,
                })
                .collect();

            let config = platform_config_with_providers(providers);
            let errors = common::config::validate_config(&config);

            let priority_errors: Vec<_> = errors
                .iter()
                .filter(|e| e.reason.contains("duplicate priority"))
                .collect();

            prop_assert!(
                !priority_errors.is_empty(),
                "Config with duplicate priorities should produce validation error"
            );
        }

        /// **Validates: Requirements 1.6**
        ///
        /// Property 2: LLM Provider Configuration Validation — Missing required fields rejected
        ///
        /// Configurations with missing required fields (empty name, empty model,
        /// empty base_url, or missing API key for non-Ollama) SHALL be rejected.
        #[test]
        fn prop_missing_required_fields_rejected(
            missing_field in prop_oneof![
                Just("name"),
                Just("model"),
                Just("base_url"),
                Just("api_key"),
            ]
        ) {
            let provider = match missing_field {
                "name" => ProviderConfig {
                    name: "".to_string(),
                    provider_type: ProviderType::Ollama,
                    api_key: None,
                    base_url: "http://localhost:11434".to_string(),
                    model: "llama3".to_string(),
                    priority: 1,
                    timeout_seconds: 30,
                    max_tokens: None,
                },
                "model" => ProviderConfig {
                    name: "test-provider".to_string(),
                    provider_type: ProviderType::Ollama,
                    api_key: None,
                    base_url: "http://localhost:11434".to_string(),
                    model: "".to_string(),
                    priority: 1,
                    timeout_seconds: 30,
                    max_tokens: None,
                },
                "base_url" => ProviderConfig {
                    name: "test-provider".to_string(),
                    provider_type: ProviderType::Ollama,
                    api_key: None,
                    base_url: "".to_string(),
                    model: "llama3".to_string(),
                    priority: 1,
                    timeout_seconds: 30,
                    max_tokens: None,
                },
                "api_key" => ProviderConfig {
                    name: "test-provider".to_string(),
                    provider_type: ProviderType::Anthropic,
                    api_key: None, // Missing for non-Ollama
                    base_url: "https://api.anthropic.com".to_string(),
                    model: "claude-3".to_string(),
                    priority: 1,
                    timeout_seconds: 30,
                    max_tokens: None,
                },
                _ => unreachable!(),
            };

            let config = platform_config_with_providers(vec![provider]);
            let errors = common::config::validate_config(&config);

            let llm_errors: Vec<_> = errors
                .iter()
                .filter(|e| e.setting.starts_with("llm.providers"))
                .collect();

            prop_assert!(
                !llm_errors.is_empty(),
                "Config with missing '{}' should produce validation error",
                missing_field
            );
        }
    }
}
