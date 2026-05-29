//! Integration tests for the Agent Core runtime.
//!
//! Tests lifecycle management, message processing, tool execution loop,
//! and agent mode switching.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;

use agent_core::{AgentCore, AgentCoreConfig, AgentMode, AgentStatus, Tool, ToolRegistry};
use common::config::{LlmConfig, ProviderConfig, ProviderType};
use common::models::{Message, MessageRole};
use common::types::{MessageId, SessionId};
use llm_router::LlmRouter;

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a minimal LLM Router config (uses Ollama which won't actually connect in tests).
fn test_llm_config() -> LlmConfig {
    LlmConfig {
        providers: vec![ProviderConfig {
            name: "test-provider".to_string(),
            provider_type: ProviderType::Ollama,
            api_key: None,
            base_url: "http://localhost:11434".to_string(),
            model: "test-model".to_string(),
            priority: 1,
            timeout_seconds: 5,
            max_tokens: None,
        }],
    }
}

/// Create a test AgentCore with default config.
fn create_test_agent() -> AgentCore {
    let router = LlmRouter::from_config(&test_llm_config());
    let registry = ToolRegistry::new();
    let config = AgentCoreConfig::default();
    AgentCore::new(router, registry, config)
}

/// A simple echo tool for testing.
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes the input message"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let msg = arguments
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("no message");
        Ok(format!("Echo: {}", msg))
    }
}

/// A coding-only tool for testing mode filtering.
struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read a file (coding mode only)"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        Ok(format!("File contents of: {}", path))
    }

    fn coding_only(&self) -> bool {
        true
    }
}

/// Create a test message.
fn make_test_message(session_id: SessionId, content: &str) -> Message {
    Message {
        id: MessageId::new(),
        session_id,
        role: MessageRole::User,
        content: content.to_string(),
        tool_calls: None,
        tool_results: None,
        timestamp: chrono::Utc::now(),
        token_count: content.len() as u32,
    }
}

// =============================================================================
// Lifecycle Tests
// =============================================================================

#[tokio::test]
async fn test_agent_starts_in_starting_state() {
    let router = LlmRouter::from_config(&test_llm_config());
    let registry = ToolRegistry::new();
    let config = AgentCoreConfig::default();
    let agent = AgentCore::new(router, registry, config);

    // Before start(), agent should be in Starting state
    let status = agent.status().await;
    assert!(matches!(status, AgentStatus::Starting));
    assert!(!agent.is_accepting_requests().await);
}

#[tokio::test]
async fn test_agent_transitions_to_idle_after_start() {
    let agent = create_test_agent();
    agent.start().await;

    let status = agent.status().await;
    assert!(matches!(status, AgentStatus::Idle));
    assert!(agent.is_accepting_requests().await);
}

#[tokio::test]
async fn test_agent_shutdown_transitions_to_shutting_down() {
    let agent = create_test_agent();
    agent.start().await;

    let result = agent.shutdown().await;
    assert!(result.is_ok());

    let status = agent.status().await;
    assert!(matches!(status, AgentStatus::ShuttingDown));
    assert!(!agent.is_accepting_requests().await);
}

#[tokio::test]
async fn test_agent_restart_returns_to_idle() {
    let agent = create_test_agent();
    agent.start().await;

    let result = agent.restart().await;
    assert!(result.is_ok());

    let status = agent.status().await;
    assert!(matches!(status, AgentStatus::Idle));
    assert!(agent.is_accepting_requests().await);
}

#[tokio::test]
async fn test_agent_not_accepting_requests_before_start() {
    let agent = create_test_agent();
    assert!(!agent.is_accepting_requests().await);
}

#[tokio::test]
async fn test_agent_not_accepting_requests_after_shutdown() {
    let agent = create_test_agent();
    agent.start().await;
    agent.shutdown().await.unwrap();
    assert!(!agent.is_accepting_requests().await);
}

// =============================================================================
// Mode Switching Tests
// =============================================================================

#[tokio::test]
async fn test_default_mode_is_general() {
    let agent = create_test_agent();
    let mode = agent.mode().await;
    assert!(matches!(mode, AgentMode::General));
}

#[tokio::test]
async fn test_switch_to_coding_mode() {
    let agent = create_test_agent();
    agent.start().await;

    let result = agent
        .set_mode(AgentMode::Coding {
            workspace: "/tmp/project".into(),
            plan_only: false,
        })
        .await;
    assert!(result.is_ok());

    let mode = agent.mode().await;
    assert!(matches!(mode, AgentMode::Coding { .. }));
}

#[tokio::test]
async fn test_switch_back_to_general_mode() {
    let agent = create_test_agent();
    agent.start().await;

    agent
        .set_mode(AgentMode::Coding {
            workspace: "/tmp/project".into(),
            plan_only: false,
        })
        .await
        .unwrap();

    agent.set_mode(AgentMode::General).await.unwrap();

    let mode = agent.mode().await;
    assert!(matches!(mode, AgentMode::General));
}

// =============================================================================
// Tool Registry Integration Tests
// =============================================================================

#[tokio::test]
async fn test_tool_registration_via_agent() {
    let agent = create_test_agent();

    {
        let mut registry = agent.tool_registry().write().await;
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FileReadTool));
    }

    let registry = agent.tool_registry().read().await;
    assert_eq!(registry.tool_count(), 2);
}

#[tokio::test]
async fn test_tool_definitions_respect_mode() {
    let agent = create_test_agent();

    {
        let mut registry = agent.tool_registry().write().await;
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FileReadTool));
    }

    // In General mode, coding tools should be excluded
    let registry = agent.tool_registry().read().await;
    let general_tools = registry.tool_definitions(false, false);
    assert_eq!(general_tools.len(), 1);
    assert_eq!(general_tools[0].name, "echo");

    // In Coding mode, all tools should be included
    let coding_tools = registry.tool_definitions(true, false);
    assert_eq!(coding_tools.len(), 2);
}

// =============================================================================
// Message Processing Tests
// =============================================================================

#[tokio::test]
async fn test_process_message_rejected_when_not_accepting() {
    let agent = create_test_agent();
    // Don't call start() — agent is not accepting requests

    let session_id = SessionId::new();
    let message = make_test_message(session_id, "Hello");

    let mut stream = agent.process_message(session_id, message, vec![]).await;
    let result = stream.next().await;

    assert!(result.is_some());
    assert!(result.unwrap().is_err());
}

#[tokio::test]
async fn test_in_flight_count_starts_at_zero() {
    let agent = create_test_agent();
    assert_eq!(agent.in_flight_count().await, 0);
}

// =============================================================================
// Config Tests
// =============================================================================

#[test]
fn test_default_config_values() {
    let config = AgentCoreConfig::default();
    assert_eq!(config.max_tool_iterations, 20);
    assert_eq!(config.message_timeout_seconds, 300);
    assert_eq!(config.shutdown_timeout_seconds, 30);
    assert_eq!(config.context_history_limit, 50);
}

#[test]
fn test_agent_status_serialization() {
    let status = AgentStatus::Working {
        task: "Processing message".to_string(),
        progress: Some(0.5),
    };
    let json = serde_json::to_string(&status).unwrap();
    assert!(json.contains("working"));
    assert!(json.contains("Processing message"));

    let deserialized: AgentStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, status);
}

#[test]
fn test_agent_mode_serialization() {
    let mode = AgentMode::Coding {
        workspace: "/home/user/project".into(),
        plan_only: false,
    };
    let json = serde_json::to_string(&mode).unwrap();
    assert!(json.contains("coding"));

    let deserialized: AgentMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, mode);
}
