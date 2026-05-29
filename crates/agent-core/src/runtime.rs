//! Agent Core Runtime — the central orchestrator for all agent operations.
//!
//! Manages the agent lifecycle (startup, shutdown, graceful restart),
//! processes incoming messages through the LLM Router, coordinates tool
//! execution via the Tool Registry, and supports General/Coding agent modes.
//!
//! Requirements: 2.1, 2.4, 3.3

use std::sync::Arc;
use std::time::Duration;

use futures::stream;
use tokio::sync::{Mutex, RwLock};
use tokio::time;
use tracing::{debug, error, info, warn};

use common::errors::{LlmError, PlatformError};
use common::models::{Message, MessageRole, ToolResult};
use common::types::SessionId;
use llm_router::types::{
    ChatMessage, ChatRole, CompletionRequest, ImageContent, IMAGE_SENTINEL_KEY,
};
use llm_router::LlmRouter;

use crate::tool_registry::ToolRegistry;
use crate::types::{
    AgentCoreConfig, AgentMode, AgentStatus, ModeError, PostTaskHookFn, ResponseChunk,
    ResponseStream, ShutdownError,
};

/// The central agent runtime that orchestrates all operations.
///
/// Holds references to the LLM Router, Tool Registry, and manages
/// the agent's lifecycle and message processing loop.
pub struct AgentCore {
    /// The LLM Router for sending completion requests.
    llm_router: Arc<RwLock<LlmRouter>>,

    /// The Tool Registry for executing tool calls.
    tool_registry: Arc<RwLock<ToolRegistry>>,

    /// Current agent status.
    status: Arc<RwLock<AgentStatus>>,

    /// Current agent mode (General or Coding).
    mode: Arc<RwLock<AgentMode>>,

    /// Number of in-flight requests (for graceful shutdown).
    in_flight: Arc<Mutex<usize>>,

    /// Whether the agent is accepting new requests.
    accepting_requests: Arc<RwLock<bool>>,

    /// Runtime configuration.
    config: AgentCoreConfig,

    /// Live system prompt, mutable at runtime without a restart. Updated either
    /// via `set_system_prompt` (config API) or by writing to the slot handed out
    /// by `live_system_prompt` (skill-reflection hook). Initialised from
    /// `config.system_prompt` and read by `process_message` for each message.
    system_prompt: Arc<RwLock<Option<String>>>,

    /// Optional fire-and-forget hook called after every completed message loop.
    post_task_hook: Option<PostTaskHookFn>,
}

impl AgentCore {
    /// Create a new AgentCore with the given components.
    pub fn new(
        llm_router: LlmRouter,
        tool_registry: ToolRegistry,
        config: AgentCoreConfig,
    ) -> Self {
        let system_prompt = Arc::new(RwLock::new(config.system_prompt.clone()));
        Self {
            llm_router: Arc::new(RwLock::new(llm_router)),
            tool_registry: Arc::new(RwLock::new(tool_registry)),
            status: Arc::new(RwLock::new(AgentStatus::Starting)),
            mode: Arc::new(RwLock::new(AgentMode::General)),
            in_flight: Arc::new(Mutex::new(0)),
            accepting_requests: Arc::new(RwLock::new(false)),
            config,
            system_prompt,
            post_task_hook: None,
        }
    }

    /// Attach a post-task hook that is fired (fire-and-forget) after each
    /// completed message loop.  The hook receives the total tool-call count
    /// and the full conversation transcript.
    pub fn with_post_task_hook(mut self, hook: PostTaskHookFn) -> Self {
        self.post_task_hook = Some(hook);
        self
    }

    /// Start the agent runtime.
    ///
    /// Transitions the agent from Starting to Idle and begins accepting requests.
    pub async fn start(&self) {
        info!("Agent Core starting up");

        // Transition to Idle
        *self.status.write().await = AgentStatus::Idle;
        *self.accepting_requests.write().await = true;

        info!("Agent Core started, accepting requests");
    }

    /// Gracefully shut down the agent.
    ///
    /// Stops accepting new requests, waits for in-flight requests to drain
    /// (up to the configured timeout), then transitions to ShuttingDown.
    pub async fn shutdown(&self) -> Result<(), ShutdownError> {
        info!("Agent Core initiating graceful shutdown");

        // Stop accepting new requests
        *self.accepting_requests.write().await = false;
        *self.status.write().await = AgentStatus::ShuttingDown;

        // Wait for in-flight requests to drain
        let timeout = Duration::from_secs(self.config.shutdown_timeout_seconds);
        let start = std::time::Instant::now();

        loop {
            let pending = *self.in_flight.lock().await;
            if pending == 0 {
                info!("All in-flight requests drained, shutdown complete");
                return Ok(());
            }

            if start.elapsed() >= timeout {
                error!(
                    pending_requests = pending,
                    elapsed_seconds = start.elapsed().as_secs(),
                    "Shutdown timed out with pending requests"
                );
                return Err(ShutdownError::Timeout {
                    elapsed_seconds: start.elapsed().as_secs(),
                    pending_requests: pending,
                });
            }

            debug!(
                pending_requests = pending,
                "Waiting for in-flight requests to drain"
            );
            time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Gracefully restart the agent.
    ///
    /// Shuts down, then starts back up. In-flight requests are drained first.
    pub async fn restart(&self) -> Result<(), ShutdownError> {
        info!("Agent Core initiating graceful restart");
        self.shutdown().await?;
        self.start().await;
        info!("Agent Core restart complete");
        Ok(())
    }

    /// Get the current agent status.
    pub async fn status(&self) -> AgentStatus {
        self.status.read().await.clone()
    }

    /// Get the current agent mode.
    pub async fn mode(&self) -> AgentMode {
        self.mode.read().await.clone()
    }

    /// Switch the agent mode.
    ///
    /// Cannot switch while the agent is actively working on a request.
    pub async fn set_mode(&self, mode: AgentMode) -> Result<(), ModeError> {
        let current_status = self.status.read().await.clone();
        if matches!(current_status, AgentStatus::Working { .. }) {
            return Err(ModeError::AgentBusy);
        }

        let old_mode = self.mode.read().await.clone();
        *self.mode.write().await = mode.clone();

        info!(
            old_mode = ?old_mode,
            new_mode = ?mode,
            "Agent mode switched"
        );

        Ok(())
    }

    /// Process an incoming message and return a response stream.
    ///
    /// This is the main message processing loop:
    /// 1. Load session context from history
    /// 2. Send to LLM Router for completion
    /// 3. If response contains tool calls, execute them via ToolRegistry
    /// 4. Loop until LLM returns a final response (no more tool calls)
    /// 5. Return the final response
    ///
    /// The response is returned as a stream for real-time delivery.
    pub async fn process_message(
        &self,
        session_id: SessionId,
        message: Message,
        history: Vec<Message>,
    ) -> ResponseStream {
        // Check if we're accepting requests
        let accepting = *self.accepting_requests.read().await;
        if !accepting {
            return Box::pin(stream::once(async {
                Err(PlatformError::Llm(LlmError::AllProvidersFailed {
                    attempts: vec![],
                }))
            }));
        }

        // Track in-flight request
        {
            let mut count = self.in_flight.lock().await;
            *count += 1;
        }

        // Update status to Working
        *self.status.write().await = AgentStatus::Working {
            task: format!("Processing message for session {}", session_id),
            progress: None,
        };

        // Clone what we need for the async stream
        let llm_router = Arc::clone(&self.llm_router);
        let tool_registry = Arc::clone(&self.tool_registry);
        let status = Arc::clone(&self.status);
        let in_flight = Arc::clone(&self.in_flight);
        let mode = Arc::clone(&self.mode);
        let max_iterations = self.config.max_tool_iterations;
        // Use the live (possibly refreshed) system prompt, falling back to the
        // initial config value if it was never set.
        let system_prompt = self.system_prompt.read().await.clone();
        let post_task_hook = self.post_task_hook.clone();

        let response_stream = async move {
            let outcome = Self::run_message_loop(
                session_id,
                message,
                history,
                &llm_router,
                &tool_registry,
                &mode,
                max_iterations,
                system_prompt.as_deref(),
            )
            .await;

            // Fire post-task hook (fire-and-forget) when tool calls were made.
            if let Ok((_, tool_count, ref transcript)) = outcome {
                if tool_count > 0 {
                    if let Some(ref hook) = post_task_hook {
                        let hook = Arc::clone(hook);
                        let count = tool_count;
                        let msgs = transcript.clone();
                        tokio::spawn(async move { hook(count, msgs).await });
                    }
                }
            }

            // Extract the ResponseChunk from the outcome.
            let result = outcome.map(|(chunk, _, _)| chunk);

            // Decrement in-flight count
            {
                let mut count = in_flight.lock().await;
                *count = count.saturating_sub(1);
            }

            // Update status back to Idle (if no other requests are in-flight)
            {
                let count = *in_flight.lock().await;
                if count == 0 {
                    *status.write().await = AgentStatus::Idle;
                }
            }

            result
        };

        // Convert the future into a stream that yields the final response
        Box::pin(stream::once(response_stream))
    }

    /// The core message processing loop.
    ///
    /// Returns `(ResponseChunk, tool_call_count, transcript)`.
    /// `tool_call_count` is the total number of tool calls executed across all
    /// iterations; `transcript` is the full conversation as built up during the loop.
    async fn run_message_loop(
        session_id: SessionId,
        user_message: Message,
        history: Vec<Message>,
        llm_router: &Arc<RwLock<LlmRouter>>,
        tool_registry: &Arc<RwLock<ToolRegistry>>,
        mode: &Arc<RwLock<AgentMode>>,
        max_iterations: u32,
        system_prompt: Option<&str>,
    ) -> Result<(ResponseChunk, usize, Vec<ChatMessage>), PlatformError> {
        // Build the initial conversation from history + new message
        let mut messages = Vec::new();

        // Prepend system prompt if configured
        if let Some(prompt) = system_prompt {
            messages.push(ChatMessage::text(ChatRole::System, prompt));
        }

        messages.extend(Self::build_chat_messages(&history));
        messages.push(ChatMessage::text(
            ChatRole::User,
            user_message.content.clone(),
        ));

        // Get available tools based on current mode. Plan mode keeps `include_coding`
        // on but flips `plan_only`, which filters out destructive tools.
        let (include_coding, plan_only) = {
            let current_mode = mode.read().await;
            match &*current_mode {
                AgentMode::General => (false, false),
                AgentMode::Coding { plan_only, .. } => (true, *plan_only),
            }
        };

        let mut all_tool_results: Vec<ToolResult> = Vec::new();
        let mut iteration = 0;

        loop {
            iteration += 1;
            if iteration > max_iterations {
                warn!(
                    session_id = %session_id,
                    iterations = iteration,
                    "Max tool iterations reached, returning partial response"
                );
                let tool_count = all_tool_results.len();
                return Ok((
                    ResponseChunk {
                        content: Some(
                            "I've reached the maximum number of tool call iterations. Here's what I have so far.".to_string()
                        ),
                        done: true,
                        tool_results: if all_tool_results.is_empty() {
                            None
                        } else {
                            Some(all_tool_results)
                        },
                    },
                    tool_count,
                    messages,
                ));
            }

            // Get tool definitions
            let tools = {
                let registry = tool_registry.read().await;
                registry.tool_definitions(include_coding, plan_only)
            };

            // Build the completion request
            let request = CompletionRequest {
                messages: messages.clone(),
                tools,
                max_tokens: None,
                temperature: None,
                stream: false,
            };

            // Send to LLM Router
            debug!(
                session_id = %session_id,
                iteration = iteration,
                message_count = messages.len(),
                "Sending completion request to LLM Router"
            );

            let response = {
                let router = llm_router.read().await;
                router.complete(&request).await?
            };

            // Check if the response contains tool calls
            if response.tool_calls.is_empty() {
                // Final response — no more tool calls
                debug!(
                    session_id = %session_id,
                    iteration = iteration,
                    "LLM returned final response (no tool calls)"
                );

                let tool_count = all_tool_results.len();
                // Push the final assistant message into the transcript before returning.
                messages.push(ChatMessage::text(
                    ChatRole::Assistant,
                    response.content.clone(),
                ));
                return Ok((
                    ResponseChunk {
                        content: Some(response.content),
                        done: true,
                        tool_results: if all_tool_results.is_empty() {
                            None
                        } else {
                            Some(all_tool_results)
                        },
                    },
                    tool_count,
                    messages,
                ));
            }

            // Execute tool calls
            debug!(
                session_id = %session_id,
                iteration = iteration,
                tool_call_count = response.tool_calls.len(),
                "Executing tool calls"
            );

            // Add the assistant's response (with tool calls) to the conversation
            messages.push(ChatMessage::text(
                ChatRole::Assistant,
                response.content.clone(),
            ));

            // Execute each tool call
            let mut iteration_results = Vec::new();
            for tool_call in &response.tool_calls {
                let arguments: serde_json::Value =
                    serde_json::from_str(&tool_call.arguments).unwrap_or_default();

                let result = {
                    let registry = tool_registry.read().await;
                    registry
                        .execute_tool_call(&tool_call.id, &tool_call.name, arguments)
                        .await
                };

                iteration_results.push(result);
            }

            // Add tool results to the conversation as tool messages. A result
            // may carry an inline image (from the view_image tool) encoded as a
            // sentinel JSON object — convert that into a multimodal message.
            for result in &iteration_results {
                messages.push(tool_result_to_message(&result.output));
            }

            all_tool_results.extend(iteration_results);
        }
    }

    /// Convert stored Messages into ChatMessages for the LLM.
    fn build_chat_messages(history: &[Message]) -> Vec<ChatMessage> {
        history
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    MessageRole::User => ChatRole::User,
                    MessageRole::Assistant => ChatRole::Assistant,
                    MessageRole::System => ChatRole::System,
                    MessageRole::Tool => ChatRole::Tool,
                };
                ChatMessage::text(role, msg.content.clone())
            })
            .collect()
    }

    /// Get the current system prompt (cloned).
    pub async fn system_prompt(&self) -> Option<String> {
        self.system_prompt.read().await.clone()
    }

    /// Replace the system prompt. Takes effect on the next message processed —
    /// in-flight requests keep their snapshotted prompt.
    pub async fn set_system_prompt(&self, prompt: Option<String>) {
        let mut guard = self.system_prompt.write().await;
        *guard = prompt;
    }

    /// Get a reference to the tool registry for external registration.
    pub fn tool_registry(&self) -> &Arc<RwLock<ToolRegistry>> {
        &self.tool_registry
    }

    /// Get a reference to the LLM router.
    pub fn llm_router(&self) -> &Arc<RwLock<LlmRouter>> {
        &self.llm_router
    }

    /// Get a shared reference to the live system prompt slot.
    ///
    /// Callers may write a new prompt to this `Arc<RwLock<_>>` to update the
    /// system prompt that will be used on the next message (e.g. after a skill
    /// is created by the reflection hook).
    pub fn live_system_prompt(&self) -> Arc<RwLock<Option<String>>> {
        Arc::clone(&self.system_prompt)
    }

    /// Check if the agent is currently accepting requests.
    pub async fn is_accepting_requests(&self) -> bool {
        *self.accepting_requests.read().await
    }

    /// Get the number of in-flight requests.
    pub async fn in_flight_count(&self) -> usize {
        *self.in_flight.lock().await
    }
}

/// Convert a tool result's string output into a ChatMessage. If the output is
/// the image sentinel emitted by the `view_image` tool
/// (`{"__xeno_image__": {media_type, data, note}}`), build a multimodal message
/// carrying the image; otherwise a plain text tool message.
fn tool_result_to_message(output: &str) -> ChatMessage {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(output) {
        if let Some(img) = map.get(IMAGE_SENTINEL_KEY).and_then(|v| v.as_object()) {
            let media_type = img
                .get("media_type")
                .and_then(|v| v.as_str())
                .unwrap_or("image/png")
                .to_string();
            let data = img
                .get("data")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let note = img
                .get("note")
                .and_then(|v| v.as_str())
                .unwrap_or("Here is the requested image.")
                .to_string();
            if !data.is_empty() {
                return ChatMessage::with_images(
                    ChatRole::Tool,
                    note,
                    vec![ImageContent { media_type, data }],
                );
            }
        }
    }
    ChatMessage::text(ChatRole::Tool, output.to_string())
}
