//! Property-based tests for the LLM Router.
//!
//! **Validates: Requirements 1.4, 1.5, 1.6**
//!
//! Property 1: LLM Router Failover Correctness
//! Property 2: LLM Provider Configuration Validation
//!
//! These integration tests verify properties through the public API.
//! Property 1 (failover) is tested via the router's priority ordering
//! behavior using `from_provider_configs`.
//! Property 2 (config validation) is tested via `validate_config`.

use proptest::prelude::*;

use common::config::*;
use llm_router::LlmRouter;

// ============================================================================
// Property 1: LLM Router Failover Correctness (Priority Ordering)
//
// **Validates: Requirements 1.4, 1.5**
// ============================================================================

/// Strategy: generate 1-10 distinct priorities in random order.
fn distinct_priorities_strategy() -> impl Strategy<Value = Vec<u8>> {
    (1usize..=10).prop_flat_map(|n| {
        proptest::collection::hash_set(1u8..=255, n)
            .prop_map(|set| set.into_iter().collect::<Vec<u8>>())
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 1.4, 1.5**
    ///
    /// Property 1 (ordering aspect): For any set of providers with distinct
    /// priorities given in any order, the router SHALL sort them so that
    /// the lowest priority value is first (tried first).
    #[test]
    fn prop_router_sorts_providers_by_priority(
        priorities in distinct_priorities_strategy()
    ) {
        let configs: Vec<ProviderConfig> = priorities
            .iter()
            .enumerate()
            .map(|(i, &priority)| ProviderConfig {
                name: format!("provider-{}", i),
                provider_type: ProviderType::Ollama,
                api_key: None,
                base_url: "http://localhost:11434".to_string(),
                model: "llama3".to_string(),
                priority,
                timeout_seconds: 30,
                max_tokens: None,
            })
            .collect();

        let router = LlmRouter::from_provider_configs(&configs);
        let status = router.provider_status();

        // Verify providers are in ascending priority order
        for i in 1..status.len() {
            prop_assert!(
                status[i - 1].priority < status[i].priority,
                "Providers not in priority order: {} (p{}) before {} (p{})",
                status[i - 1].name, status[i - 1].priority,
                status[i].name, status[i].priority,
            );
        }

        // Verify all providers are present
        prop_assert_eq!(status.len(), priorities.len());
    }

    /// **Validates: Requirements 1.4, 1.5**
    ///
    /// Property 1 (count preservation): The router SHALL contain exactly
    /// the same number of providers as configured.
    #[test]
    fn prop_router_preserves_provider_count(
        priorities in distinct_priorities_strategy()
    ) {
        let configs: Vec<ProviderConfig> = priorities
            .iter()
            .enumerate()
            .map(|(i, &priority)| ProviderConfig {
                name: format!("provider-{}", i),
                provider_type: ProviderType::Ollama,
                api_key: None,
                base_url: "http://localhost:11434".to_string(),
                model: "llama3".to_string(),
                priority,
                timeout_seconds: 30,
                max_tokens: None,
            })
            .collect();

        let router = LlmRouter::from_provider_configs(&configs);
        prop_assert_eq!(router.provider_count(), priorities.len());
    }
}

// ============================================================================
// Property 2: LLM Provider Configuration Validation
//
// **Validates: Requirements 1.6**
// ============================================================================

/// Helper: create a minimal valid PlatformConfig with given LLM providers.
fn platform_config_with_providers(providers: Vec<ProviderConfig>) -> PlatformConfig {
    PlatformConfig {
        general: GeneralConfig::default(),
        llm: LlmConfig { providers },
        security: SecurityConfig::default(),
        coding: None,
        scheduler: SchedulerConfig::default(),
        web: WebConfig::default(),
        serve: ServeConfig::default(),
        messaging: MessagingConfig::default(),
        monitoring: MonitoringConfig::default(),
        plugins: PluginConfig::default(),
        mcp: McpConfig::default(),
        skills: common::config::SkillsConfig::default(),
    }
}

/// Strategy: generate a valid LLM config with 1-10 providers having
/// distinct priorities, valid names, models, and timeouts.
fn valid_provider_configs_strategy() -> impl Strategy<Value = Vec<ProviderConfig>> {
    (1usize..=10).prop_flat_map(|n| {
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
    prop_oneof![Just(0usize), 11usize..=15]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 1.6**
    ///
    /// Property 2: Valid configs with 1-10 providers, valid fields, and unique
    /// priorities SHALL be accepted (no LLM-related validation errors).
    #[test]
    fn prop_valid_llm_config_accepted(
        providers in valid_provider_configs_strategy()
    ) {
        let config = platform_config_with_providers(providers);
        let errors = validate_config(&config);

        let llm_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.setting.starts_with("llm."))
            .collect();

        prop_assert!(
            llm_errors.is_empty(),
            "Valid LLM config should produce no LLM errors, got: {:?}",
            llm_errors
        );
    }

    /// **Validates: Requirements 1.6**
    ///
    /// Property 2: Configurations with 0 or more than 10 providers SHALL
    /// be rejected with a validation error on "llm.providers".
    #[test]
    fn prop_invalid_provider_count_rejected(
        count in invalid_provider_count_strategy()
    ) {
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
        let errors = validate_config(&config);

        let provider_count_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.setting == "llm.providers")
            .collect();

        prop_assert!(
            !provider_count_errors.is_empty(),
            "Config with {} providers should produce validation error",
            count
        );
    }

    /// **Validates: Requirements 1.6**
    ///
    /// Property 2: Configurations where two or more providers share the same
    /// priority SHALL be rejected with a duplicate priority error.
    #[test]
    fn prop_duplicate_priorities_rejected(
        n in 2usize..=10,
        dup_priority in 1u8..=255u8,
    ) {
        let providers: Vec<ProviderConfig> = (0..n)
            .map(|i| ProviderConfig {
                name: format!("provider-{}", i),
                provider_type: ProviderType::Ollama,
                api_key: None,
                base_url: "http://localhost:11434".to_string(),
                model: "llama3".to_string(),
                priority: dup_priority,
                timeout_seconds: 30,
                max_tokens: None,
            })
            .collect();

        let config = platform_config_with_providers(providers);
        let errors = validate_config(&config);

        let priority_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.reason.contains("duplicate priority"))
            .collect();

        prop_assert!(
            !priority_errors.is_empty(),
            "Config with {} providers at priority {} should produce error",
            n, dup_priority
        );
    }

    /// **Validates: Requirements 1.6**
    ///
    /// Property 2: Configurations with missing required fields SHALL be rejected.
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
                api_key: None,
                base_url: "https://api.anthropic.com".to_string(),
                model: "claude-3".to_string(),
                priority: 1,
                timeout_seconds: 30,
                max_tokens: None,
            },
            _ => unreachable!(),
        };

        let config = platform_config_with_providers(vec![provider]);
        let errors = validate_config(&config);

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
