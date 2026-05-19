//! Session Manager — manages concurrent sessions with context window management.
//!
//! Provides independent context tracking for multiple concurrent sessions,
//! token usage monitoring, and automatic context summarization when token
//! usage exceeds 80% of the configured LLM token limit.
//!
//! Supports optional SQLite persistence via memory-store for session state
//! recovery across restarts.
//!
//! Requirements: 14.2, 14.4

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use sqlx::sqlite::SqlitePool;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use common::config::SchedulerConfig;
use common::models::{Message, MessageRole};
use common::types::{MessageId, SessionId};

/// Number of most recent conversation turns preserved in full after summarization.
const PRESERVED_TURNS: usize = 10;

/// Threshold ratio of token usage to token limit that triggers summarization.
const SUMMARIZATION_THRESHOLD: f64 = 0.80;

/// Maximum number of concurrent sessions supported.
pub const MAX_CONCURRENT_SESSIONS: usize = 50;

/// Errors specific to session management.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The maximum number of concurrent sessions has been reached.
    #[error("Maximum concurrent sessions ({max}) reached")]
    MaxSessionsReached { max: usize },

    /// The requested session was not found.
    #[error("Session not found: {id}")]
    SessionNotFound { id: String },
}

/// Configuration for the Session Manager.
#[derive(Debug, Clone)]
pub struct SessionManagerConfig {
    /// The maximum token limit for the configured LLM.
    pub token_limit: u32,
    /// Maximum number of concurrent sessions (default: 50).
    pub max_sessions: usize,
}

impl Default for SessionManagerConfig {
    fn default() -> Self {
        Self {
            token_limit: 128_000,
            max_sessions: MAX_CONCURRENT_SESSIONS,
        }
    }
}

/// A knowledge entry preserved across summarization.
#[derive(Debug, Clone)]
pub struct KnowledgeEntry {
    /// Title or key for the knowledge.
    pub title: String,
    /// The knowledge content.
    pub content: String,
}

/// The context for a single session, including messages and knowledge.
#[derive(Debug, Clone)]
pub struct SessionContext {
    /// All messages in this session's context window.
    messages: Vec<Message>,
    /// Stored knowledge entries that persist across summarization.
    knowledge: Vec<KnowledgeEntry>,
    /// Total token count for all messages in the context.
    total_tokens: u32,
    /// Whether a summary has been applied to this session.
    has_summary: bool,
}

impl SessionContext {
    /// Create a new empty session context.
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            knowledge: Vec::new(),
            total_tokens: 0,
            has_summary: false,
        }
    }

    /// Get the total token count for this session.
    pub fn token_count(&self) -> u32 {
        self.total_tokens
    }

    /// Get all messages in this session's context.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Get all knowledge entries for this session.
    pub fn knowledge(&self) -> &[KnowledgeEntry] {
        &self.knowledge
    }

    /// Whether this session has had context summarized.
    pub fn has_summary(&self) -> bool {
        self.has_summary
    }
}

/// The Session Manager manages concurrent sessions with independent context,
/// tracks token usage, and performs automatic context summarization.
///
/// Optionally persists session state to SQLite via memory-store functions.
/// Thread-safe via `RwLock` for concurrent access from multiple tasks.
pub struct SessionManager {
    /// Map of session ID to session context, protected by RwLock.
    sessions: Arc<RwLock<HashMap<SessionId, SessionContext>>>,
    /// Configuration for token limits and session capacity.
    config: SessionManagerConfig,
    /// Optional SQLite pool for session state persistence.
    db_pool: Option<SqlitePool>,
}

impl SessionManager {
    /// Create a new SessionManager with the given configuration.
    pub fn new(config: SessionManagerConfig) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
            db_pool: None,
        }
    }

    /// Create a new SessionManager with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(SessionManagerConfig::default())
    }

    /// Create a new SessionManager with a SQLite pool for persistence.
    pub fn with_persistence(config: SessionManagerConfig, pool: SqlitePool) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
            db_pool: Some(pool),
        }
    }

    /// Persist the current session state to SQLite (if a pool is configured).
    ///
    /// Builds a `SessionState` from the in-memory context and writes it
    /// to the database. Errors are logged but do not propagate — the session
    /// continues operating in-memory if persistence fails.
    pub async fn persist_session(&self, session_id: SessionId) {
        let pool = match &self.db_pool {
            Some(p) => p,
            None => return,
        };

        let sessions = self.sessions.read().await;
        let ctx = match sessions.get(&session_id) {
            Some(c) => c,
            None => return,
        };

        // Build a conversation context summary from messages
        let conversation_context = ctx
            .messages
            .iter()
            .take(5)
            .map(|m| format!("[{:?}] {}", m.role, truncate_content(&m.content, 200)))
            .collect::<Vec<_>>()
            .join("\n");

        let state = memory_store::session_state::SessionState::new(
            session_id,
            Vec::new(), // task queue managed externally
            conversation_context,
            SchedulerConfig::default(),
        );

        if let Err(e) = memory_store::session_state::save_session_state(pool, &state).await {
            warn!(
                session_id = %session_id,
                error = %e,
                "Failed to persist session state to SQLite"
            );
        } else {
            debug!(session_id = %session_id, "Session state persisted to SQLite");
        }
    }

    /// Restore a session's context from SQLite persistence (if available).
    ///
    /// If a persisted state exists, creates the session and populates it
    /// with a system message containing the restored conversation context.
    /// Returns `true` if a session was restored, `false` otherwise.
    pub async fn restore_session(&self, session_id: SessionId) -> bool {
        let pool = match &self.db_pool {
            Some(p) => p,
            None => return false,
        };

        let restored = match memory_store::session_state::restore_session_state(pool, session_id).await {
            Ok(Some(state)) => state,
            Ok(None) => return false,
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    error = %e,
                    "Failed to restore session state from SQLite"
                );
                return false;
            }
        };

        // Create the session and populate with restored context
        if self.create_session(session_id).await.is_err() {
            return false;
        }

        if !restored.conversation_context.is_empty() {
            let mut sessions = self.sessions.write().await;
            if let Some(ctx) = sessions.get_mut(&session_id) {
                let summary_msg = Message {
                    id: MessageId::new(),
                    session_id,
                    role: MessageRole::System,
                    content: format!(
                        "[Restored Context] {}",
                        restored.conversation_context
                    ),
                    tool_calls: None,
                    tool_results: None,
                    timestamp: Utc::now(),
                    token_count: Self::estimate_tokens(&restored.conversation_context),
                };
                ctx.total_tokens += summary_msg.token_count;
                ctx.messages.push(summary_msg);
            }
        }

        info!(session_id = %session_id, "Session restored from SQLite persistence");
        true
    }

    /// Get the context for a session.
    ///
    /// Returns a clone of the session's current context including all messages
    /// and knowledge entries. Creates a new session if one doesn't exist and
    /// capacity allows.
    ///
    /// # Errors
    /// - `SessionError::MaxSessionsReached` if creating a new session would
    ///   exceed the configured maximum.
    pub async fn get_context(
        &self,
        session_id: SessionId,
    ) -> Result<SessionContext, SessionError> {
        let sessions = self.sessions.read().await;
        if let Some(ctx) = sessions.get(&session_id) {
            return Ok(ctx.clone());
        }
        drop(sessions);

        // Session doesn't exist — try to create it
        self.create_session(session_id).await?;

        let sessions = self.sessions.read().await;
        Ok(sessions
            .get(&session_id)
            .cloned()
            .unwrap_or_else(SessionContext::new))
    }

    /// Add a message to a session's context.
    ///
    /// If the session doesn't exist, it will be created (capacity permitting).
    /// After adding the message, checks if token usage exceeds 80% of the
    /// configured limit and triggers summarization if needed.
    ///
    /// # Errors
    /// - `SessionError::MaxSessionsReached` if creating a new session would
    ///   exceed the configured maximum.
    pub async fn add_message(
        &self,
        session_id: SessionId,
        message: Message,
    ) -> Result<(), SessionError> {
        // Ensure session exists
        {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(&session_id) {
                drop(sessions);
                self.create_session(session_id).await?;
            }
        }

        let token_count = message.token_count;

        // Add the message
        {
            let mut sessions = self.sessions.write().await;
            if let Some(ctx) = sessions.get_mut(&session_id) {
                ctx.total_tokens += token_count;
                ctx.messages.push(message);
            }
        }

        // Check if summarization is needed
        self.maybe_summarize(session_id).await;

        Ok(())
    }

    /// Get the current token count for a session.
    ///
    /// Returns 0 if the session doesn't exist.
    pub async fn get_token_count(&self, session_id: SessionId) -> u32 {
        let sessions = self.sessions.read().await;
        sessions
            .get(&session_id)
            .map(|ctx| ctx.total_tokens)
            .unwrap_or(0)
    }

    /// Store a knowledge entry for a session.
    ///
    /// Knowledge entries are preserved across summarization and always
    /// included in the context.
    pub async fn add_knowledge(
        &self,
        session_id: SessionId,
        entry: KnowledgeEntry,
    ) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;
        if let Some(ctx) = sessions.get_mut(&session_id) {
            ctx.knowledge.push(entry);
            Ok(())
        } else {
            Err(SessionError::SessionNotFound {
                id: session_id.to_string(),
            })
        }
    }

    /// Get the number of active sessions.
    pub async fn active_session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Remove a session, freeing its resources.
    pub async fn remove_session(&self, session_id: SessionId) -> bool {
        self.sessions.write().await.remove(&session_id).is_some()
    }

    /// Get the configured token limit.
    pub fn token_limit(&self) -> u32 {
        self.config.token_limit
    }

    /// Get the summarization threshold (token count that triggers summarization).
    pub fn summarization_threshold(&self) -> u32 {
        (self.config.token_limit as f64 * SUMMARIZATION_THRESHOLD) as u32
    }

    /// Create a new session if capacity allows.
    async fn create_session(&self, session_id: SessionId) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;

        // Double-check after acquiring write lock (another task may have created it)
        if sessions.contains_key(&session_id) {
            return Ok(());
        }

        if sessions.len() >= self.config.max_sessions {
            return Err(SessionError::MaxSessionsReached {
                max: self.config.max_sessions,
            });
        }

        sessions.insert(session_id, SessionContext::new());
        debug!(session_id = %session_id, "Created new session context");
        Ok(())
    }

    /// Check if a session needs summarization and perform it if so.
    ///
    /// Summarization is triggered when token usage exceeds 80% of the
    /// configured LLM token limit. The summarization:
    /// - Preserves the last 10 conversation turns in full
    /// - Preserves all stored knowledge entries
    /// - Replaces older messages with a single summary message
    async fn maybe_summarize(&self, session_id: SessionId) {
        let threshold = self.summarization_threshold();

        let needs_summarization = {
            let sessions = self.sessions.read().await;
            sessions
                .get(&session_id)
                .map(|ctx| ctx.total_tokens > threshold && ctx.messages.len() > PRESERVED_TURNS)
                .unwrap_or(false)
        };

        if needs_summarization {
            self.summarize_context(session_id).await;
        }
    }

    /// Perform context summarization on a session.
    ///
    /// Splits messages into:
    /// - Older messages (to be summarized)
    /// - Recent messages (last 10 turns, preserved in full)
    ///
    /// The older messages are replaced with a single system message
    /// containing a summary of their content.
    async fn summarize_context(&self, session_id: SessionId) {
        let mut sessions = self.sessions.write().await;
        let ctx = match sessions.get_mut(&session_id) {
            Some(ctx) => ctx,
            None => return,
        };

        let total_messages = ctx.messages.len();
        if total_messages <= PRESERVED_TURNS {
            return;
        }

        // Split: older messages to summarize, recent to preserve
        let split_point = total_messages - PRESERVED_TURNS;
        let older_messages: Vec<Message> = ctx.messages.drain(..split_point).collect();

        info!(
            session_id = %session_id,
            summarized_count = older_messages.len(),
            preserved_count = ctx.messages.len(),
            "Summarizing older context"
        );

        // Generate summary from older messages
        let summary_text = Self::generate_summary(&older_messages);
        let summary_token_count = Self::estimate_tokens(&summary_text);

        // Calculate token count for preserved messages
        let preserved_tokens: u32 = ctx.messages.iter().map(|m| m.token_count).sum();

        // Create a summary message and prepend it
        let summary_message = Message {
            id: MessageId::new(),
            session_id,
            role: MessageRole::System,
            content: summary_text,
            tool_calls: None,
            tool_results: None,
            timestamp: Utc::now(),
            token_count: summary_token_count,
        };

        // Insert summary at the beginning, before preserved messages
        ctx.messages.insert(0, summary_message);

        // Update total token count
        ctx.total_tokens = summary_token_count + preserved_tokens;
        ctx.has_summary = true;

        debug!(
            session_id = %session_id,
            new_token_count = ctx.total_tokens,
            message_count = ctx.messages.len(),
            "Context summarization complete"
        );
    }

    /// Generate a text summary from a list of older messages.
    ///
    /// This produces a structured summary that captures the key points
    /// of the conversation. In a production system, this would call the
    /// LLM for a proper summary; here we produce a deterministic summary
    /// from the message content for testability and offline operation.
    fn generate_summary(messages: &[Message]) -> String {
        let mut summary_parts: Vec<String> = Vec::new();

        summary_parts.push(
            "[Context Summary] The following is a summary of the earlier conversation:".to_string(),
        );

        // Group messages by role for a structured summary
        let user_messages: Vec<&Message> = messages
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .collect();
        let assistant_messages: Vec<&Message> = messages
            .iter()
            .filter(|m| m.role == MessageRole::Assistant)
            .collect();
        let tool_messages: Vec<&Message> = messages
            .iter()
            .filter(|m| m.role == MessageRole::Tool)
            .collect();

        if !user_messages.is_empty() {
            summary_parts.push(format!(
                "User discussed {} topic(s):",
                user_messages.len()
            ));
            // Include abbreviated content from each user message
            for msg in user_messages.iter().take(20) {
                let abbreviated = if msg.content.len() > 100 {
                    format!("{}...", &msg.content[..100])
                } else {
                    msg.content.clone()
                };
                summary_parts.push(format!("- {}", abbreviated));
            }
            if user_messages.len() > 20 {
                summary_parts.push(format!(
                    "  ...and {} more messages",
                    user_messages.len() - 20
                ));
            }
        }

        if !assistant_messages.is_empty() {
            summary_parts.push(format!(
                "Assistant provided {} response(s).",
                assistant_messages.len()
            ));
        }

        if !tool_messages.is_empty() {
            summary_parts.push(format!(
                "{} tool execution(s) were performed.",
                tool_messages.len()
            ));
        }

        summary_parts.join("\n")
    }

    /// Estimate token count for a string.
    ///
    /// Uses a simple heuristic of ~4 characters per token (common for English text).
    /// In production, this would use the actual tokenizer for the configured model.
    fn estimate_tokens(text: &str) -> u32 {
        // Approximate: 1 token ≈ 4 characters for English text
        (text.len() as f64 / 4.0).ceil() as u32
    }
}

/// Truncate a string to a maximum length, appending "..." if truncated.
fn truncate_content(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a test message with a given token count.
    fn make_msg(session_id: SessionId, role: MessageRole, content: &str, tokens: u32) -> Message {
        Message {
            id: MessageId::new(),
            session_id,
            role,
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            timestamp: Utc::now(),
            token_count: tokens,
        }
    }

    #[tokio::test]
    async fn test_create_session_on_get_context() {
        let mgr = SessionManager::with_defaults();
        let sid = SessionId::new();

        let ctx = mgr.get_context(sid).await.unwrap();
        assert!(ctx.messages().is_empty());
        assert_eq!(ctx.token_count(), 0);
        assert_eq!(mgr.active_session_count().await, 1);
    }

    #[tokio::test]
    async fn test_add_message_tracks_tokens() {
        let mgr = SessionManager::with_defaults();
        let sid = SessionId::new();

        let msg = make_msg(sid, MessageRole::User, "Hello", 10);
        mgr.add_message(sid, msg).await.unwrap();

        assert_eq!(mgr.get_token_count(sid).await, 10);

        let msg2 = make_msg(sid, MessageRole::Assistant, "Hi there", 15);
        mgr.add_message(sid, msg2).await.unwrap();

        assert_eq!(mgr.get_token_count(sid).await, 25);
    }

    #[tokio::test]
    async fn test_session_isolation() {
        let mgr = SessionManager::with_defaults();
        let sid_a = SessionId::new();
        let sid_b = SessionId::new();

        let msg_a = make_msg(sid_a, MessageRole::User, "Session A", 10);
        let msg_b = make_msg(sid_b, MessageRole::User, "Session B", 20);

        mgr.add_message(sid_a, msg_a).await.unwrap();
        mgr.add_message(sid_b, msg_b).await.unwrap();

        let ctx_a = mgr.get_context(sid_a).await.unwrap();
        let ctx_b = mgr.get_context(sid_b).await.unwrap();

        assert_eq!(ctx_a.messages().len(), 1);
        assert_eq!(ctx_b.messages().len(), 1);
        assert_eq!(ctx_a.messages()[0].content, "Session A");
        assert_eq!(ctx_b.messages()[0].content, "Session B");
        assert_eq!(ctx_a.token_count(), 10);
        assert_eq!(ctx_b.token_count(), 20);
    }

    #[tokio::test]
    async fn test_max_sessions_enforced() {
        let config = SessionManagerConfig {
            token_limit: 128_000,
            max_sessions: 3,
        };
        let mgr = SessionManager::new(config);

        // Create 3 sessions successfully
        for _ in 0..3 {
            let sid = SessionId::new();
            mgr.get_context(sid).await.unwrap();
        }

        // 4th session should fail
        let sid = SessionId::new();
        let result = mgr.get_context(sid).await;
        assert!(matches!(result, Err(SessionError::MaxSessionsReached { max: 3 })));
    }

    #[tokio::test]
    async fn test_supports_50_concurrent_sessions() {
        let mgr = SessionManager::with_defaults();

        // Create 50 sessions
        let mut session_ids = Vec::new();
        for _ in 0..50 {
            let sid = SessionId::new();
            mgr.get_context(sid).await.unwrap();
            session_ids.push(sid);
        }

        assert_eq!(mgr.active_session_count().await, 50);

        // Each session should be independently accessible
        for sid in &session_ids {
            let msg = make_msg(*sid, MessageRole::User, "test", 5);
            mgr.add_message(*sid, msg).await.unwrap();
        }

        // Verify each session has exactly 1 message
        for sid in &session_ids {
            let ctx = mgr.get_context(*sid).await.unwrap();
            assert_eq!(ctx.messages().len(), 1);
        }
    }

    #[tokio::test]
    async fn test_summarization_triggered_at_80_percent() {
        // Set a low token limit so we can easily trigger summarization
        let config = SessionManagerConfig {
            token_limit: 100,
            max_sessions: 50,
        };
        let mgr = SessionManager::new(config);
        let sid = SessionId::new();

        // Add 15 messages, each with 6 tokens = 90 total (exceeds 80% of 100 = 80)
        for i in 0..15 {
            let msg = make_msg(sid, MessageRole::User, &format!("Message {}", i), 6);
            mgr.add_message(sid, msg).await.unwrap();
        }

        let ctx = mgr.get_context(sid).await.unwrap();

        // After summarization: 1 summary message + 10 preserved = 11 messages
        assert_eq!(ctx.messages().len(), 11);
        assert!(ctx.has_summary());

        // First message should be the summary (System role)
        assert_eq!(ctx.messages()[0].role, MessageRole::System);
        assert!(ctx.messages()[0].content.contains("[Context Summary]"));

        // Last 10 messages should be the preserved recent ones
        // They were messages 5..15 (indices), content "Message 5" through "Message 14"
        assert_eq!(ctx.messages()[1].content, "Message 5");
        assert_eq!(ctx.messages()[10].content, "Message 14");
    }

    #[tokio::test]
    async fn test_no_summarization_below_threshold() {
        let config = SessionManagerConfig {
            token_limit: 1000,
            max_sessions: 50,
        };
        let mgr = SessionManager::new(config);
        let sid = SessionId::new();

        // Add 15 messages with 10 tokens each = 150 total (below 80% of 1000 = 800)
        for i in 0..15 {
            let msg = make_msg(sid, MessageRole::User, &format!("Message {}", i), 10);
            mgr.add_message(sid, msg).await.unwrap();
        }

        let ctx = mgr.get_context(sid).await.unwrap();

        // No summarization should have occurred
        assert_eq!(ctx.messages().len(), 15);
        assert!(!ctx.has_summary());
        assert_eq!(ctx.token_count(), 150);
    }

    #[tokio::test]
    async fn test_summarization_preserves_last_10_turns() {
        let config = SessionManagerConfig {
            token_limit: 100,
            max_sessions: 50,
        };
        let mgr = SessionManager::new(config);
        let sid = SessionId::new();

        // Add 20 messages with 5 tokens each = 100 total (exceeds 80% of 100 = 80)
        for i in 0..20 {
            let msg = make_msg(sid, MessageRole::User, &format!("Turn {}", i), 5);
            mgr.add_message(sid, msg).await.unwrap();
        }

        let ctx = mgr.get_context(sid).await.unwrap();

        // The last 10 turns should be preserved in full
        // After summary message at index 0, preserved messages are at indices 1..=10
        let preserved = &ctx.messages()[1..];
        assert_eq!(preserved.len(), 10);

        // Verify the preserved messages are the last 10 (Turn 10 through Turn 19)
        for (i, msg) in preserved.iter().enumerate() {
            assert_eq!(msg.content, format!("Turn {}", i + 10));
        }
    }

    #[tokio::test]
    async fn test_knowledge_preserved_after_summarization() {
        let config = SessionManagerConfig {
            token_limit: 100,
            max_sessions: 50,
        };
        let mgr = SessionManager::new(config);
        let sid = SessionId::new();

        // Create session and add knowledge
        mgr.get_context(sid).await.unwrap();
        mgr.add_knowledge(
            sid,
            KnowledgeEntry {
                title: "User Preference".to_string(),
                content: "Prefers concise responses".to_string(),
            },
        )
        .await
        .unwrap();
        mgr.add_knowledge(
            sid,
            KnowledgeEntry {
                title: "Project Info".to_string(),
                content: "Working on Rust project".to_string(),
            },
        )
        .await
        .unwrap();

        // Trigger summarization by exceeding token threshold
        for i in 0..15 {
            let msg = make_msg(sid, MessageRole::User, &format!("Msg {}", i), 6);
            mgr.add_message(sid, msg).await.unwrap();
        }

        let ctx = mgr.get_context(sid).await.unwrap();

        // Knowledge should still be present
        assert_eq!(ctx.knowledge().len(), 2);
        assert_eq!(ctx.knowledge()[0].title, "User Preference");
        assert_eq!(ctx.knowledge()[1].title, "Project Info");
    }

    #[tokio::test]
    async fn test_token_count_reduced_after_summarization() {
        let config = SessionManagerConfig {
            token_limit: 100,
            max_sessions: 50,
        };
        let mgr = SessionManager::new(config);
        let sid = SessionId::new();

        // Add messages to exceed threshold
        for i in 0..15 {
            let msg = make_msg(sid, MessageRole::User, &format!("Message {}", i), 6);
            mgr.add_message(sid, msg).await.unwrap();
        }

        let token_count = mgr.get_token_count(sid).await;

        // Token count should be less than the original 90 (15 * 6)
        // because summarization replaces older messages with a shorter summary
        assert!(token_count < 90, "Token count {} should be less than 90", token_count);

        // Token count should include the preserved messages (10 * 6 = 60) + summary tokens
        let preserved_tokens = 10 * 6u32;
        assert!(
            token_count >= preserved_tokens,
            "Token count {} should be at least {} (preserved messages)",
            token_count,
            preserved_tokens
        );
    }

    #[tokio::test]
    async fn test_remove_session() {
        let mgr = SessionManager::with_defaults();
        let sid = SessionId::new();

        mgr.get_context(sid).await.unwrap();
        assert_eq!(mgr.active_session_count().await, 1);

        let removed = mgr.remove_session(sid).await;
        assert!(removed);
        assert_eq!(mgr.active_session_count().await, 0);

        // Removing again returns false
        let removed = mgr.remove_session(sid).await;
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_remove_session_frees_capacity() {
        let config = SessionManagerConfig {
            token_limit: 128_000,
            max_sessions: 2,
        };
        let mgr = SessionManager::new(config);

        let sid1 = SessionId::new();
        let sid2 = SessionId::new();
        let sid3 = SessionId::new();

        mgr.get_context(sid1).await.unwrap();
        mgr.get_context(sid2).await.unwrap();

        // At capacity
        assert!(mgr.get_context(sid3).await.is_err());

        // Remove one session
        mgr.remove_session(sid1).await;

        // Now we can create a new one
        mgr.get_context(sid3).await.unwrap();
        assert_eq!(mgr.active_session_count().await, 2);
    }

    #[tokio::test]
    async fn test_get_token_count_nonexistent_session() {
        let mgr = SessionManager::with_defaults();
        let sid = SessionId::new();

        // Non-existent session returns 0
        assert_eq!(mgr.get_token_count(sid).await, 0);
    }

    #[tokio::test]
    async fn test_summarization_threshold_calculation() {
        let config = SessionManagerConfig {
            token_limit: 128_000,
            max_sessions: 50,
        };
        let mgr = SessionManager::new(config);

        // 80% of 128,000 = 102,400
        assert_eq!(mgr.summarization_threshold(), 102_400);
    }

    #[tokio::test]
    async fn test_summary_message_contains_conversation_info() {
        let config = SessionManagerConfig {
            token_limit: 100,
            max_sessions: 50,
        };
        let mgr = SessionManager::new(config);
        let sid = SessionId::new();

        // Add a mix of user and assistant messages
        for i in 0..12 {
            let role = if i % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            let msg = make_msg(sid, role, &format!("Content {}", i), 7);
            mgr.add_message(sid, msg).await.unwrap();
        }

        let ctx = mgr.get_context(sid).await.unwrap();
        let summary = &ctx.messages()[0];

        assert_eq!(summary.role, MessageRole::System);
        assert!(summary.content.contains("[Context Summary]"));
        assert!(summary.content.contains("User discussed"));
        assert!(summary.content.contains("Assistant provided"));
    }

    #[tokio::test]
    async fn test_concurrent_access_safety() {
        let mgr = Arc::new(SessionManager::with_defaults());
        let sid = SessionId::new();

        // Create the session first
        mgr.get_context(sid).await.unwrap();

        // Spawn multiple tasks that add messages concurrently
        let mut handles = Vec::new();
        for i in 0..10 {
            let mgr_clone = Arc::clone(&mgr);
            let handle = tokio::spawn(async move {
                let msg = make_msg(sid, MessageRole::User, &format!("Concurrent {}", i), 5);
                mgr_clone.add_message(sid, msg).await.unwrap();
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // All 10 messages should be present
        let ctx = mgr.get_context(sid).await.unwrap();
        assert_eq!(ctx.messages().len(), 10);
        assert_eq!(ctx.token_count(), 50);
    }
}
