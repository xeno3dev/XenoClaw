//! Agent message handler for messaging platform integration.
//!
//! Routes incoming messages through the session router to ensure each
//! platform user gets their own isolated session with separate history
//! and context.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use crate::session_router::SessionRouter;
use crate::{IncomingMessage, ManagementCommand, MessageHandler, MessagingError};

/// A MessageHandler that routes messages through the session router.
///
/// Each platform user gets their own isolated session. In a full implementation,
/// this would hold an `Arc<AgentCore>` and route messages through the agent's
/// processing pipeline with the appropriate session context.
pub struct AgentMessageHandler {
    session_router: Arc<SessionRouter>,
    // In a full implementation, this would hold an Arc<AgentCore>
    // For now, we provide a simple echo/placeholder response
}

impl AgentMessageHandler {
    /// Create a new AgentMessageHandler with the given session router.
    pub fn new(session_router: Arc<SessionRouter>) -> Self {
        Self { session_router }
    }

    /// Get a reference to the underlying session router.
    pub fn session_router(&self) -> &Arc<SessionRouter> {
        &self.session_router
    }
}

#[async_trait]
impl MessageHandler for AgentMessageHandler {
    async fn handle_message(&self, message: IncomingMessage) -> Result<String, MessagingError> {
        // 1. Get or create session for this user
        let session_id = self
            .session_router
            .get_or_create_session(message.platform, &message.platform_user_id)
            .await;

        debug!(
            platform = ?message.platform,
            user_id = %message.platform_user_id,
            session_id = %session_id,
            "Routing message through session"
        );

        // 2. Route message through agent core with this session
        // For now, return a placeholder indicating the session was found/created.
        // In a full implementation, this would call:
        //   self.agent_core.process_message(session_id, message).await
        Ok(format!(
            "[Session {}] Received: {}",
            session_id, message.content
        ))
    }

    async fn handle_command(
        &self,
        command: ManagementCommand,
        _from: &IncomingMessage,
    ) -> Result<String, MessagingError> {
        // Handle management commands
        match command {
            ManagementCommand::Status => Ok("Agent status: running".to_string()),
            ManagementCommand::Tasks => Ok("No scheduled tasks".to_string()),
            ManagementCommand::Mode(mode) => Ok(format!(
                "Mode: {}",
                mode.unwrap_or_else(|| "general".to_string())
            )),
            ManagementCommand::Schedule(schedule) => {
                Ok(format!("Schedule acknowledged: {}", schedule))
            }
            ManagementCommand::Config(key, value) => match (key, value) {
                (Some(k), Some(v)) => Ok(format!("Config set: {} = {}", k, v)),
                (Some(k), None) => Ok(format!("Config get: {}", k)),
                _ => Ok("Config: use !config <key> [value]".to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_message(platform: crate::Platform, user_id: &str, content: &str) -> IncomingMessage {
        IncomingMessage {
            platform,
            platform_user_id: user_id.to_string(),
            channel_id: None,
            content: content.to_string(),
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_handle_message_creates_session() {
        let router = Arc::new(SessionRouter::new());
        let handler = AgentMessageHandler::new(Arc::clone(&router));

        let msg = make_message(crate::Platform::Telegram, "user1", "hello");
        let response = handler.handle_message(msg).await.unwrap();

        assert!(response.contains("[Session"));
        assert!(response.contains("Received: hello"));

        // Session should now exist
        let session = router.get_session(crate::Platform::Telegram, "user1").await;
        assert!(session.is_some());
    }

    #[tokio::test]
    async fn test_handle_message_reuses_session() {
        let router = Arc::new(SessionRouter::new());
        let handler = AgentMessageHandler::new(Arc::clone(&router));

        let msg1 = make_message(crate::Platform::Telegram, "user1", "first");
        let msg2 = make_message(crate::Platform::Telegram, "user1", "second");

        let response1 = handler.handle_message(msg1).await.unwrap();
        let response2 = handler.handle_message(msg2).await.unwrap();

        // Extract session IDs from responses — they should be the same
        let session_id_1: String = response1
            .strip_prefix("[Session ")
            .unwrap()
            .split(']')
            .next()
            .unwrap()
            .to_string();
        let session_id_2: String = response2
            .strip_prefix("[Session ")
            .unwrap()
            .split(']')
            .next()
            .unwrap()
            .to_string();

        assert_eq!(session_id_1, session_id_2);
    }

    #[tokio::test]
    async fn test_different_users_get_different_sessions() {
        let router = Arc::new(SessionRouter::new());
        let handler = AgentMessageHandler::new(Arc::clone(&router));

        let msg1 = make_message(crate::Platform::Telegram, "user1", "hello");
        let msg2 = make_message(crate::Platform::Telegram, "user2", "hello");

        let response1 = handler.handle_message(msg1).await.unwrap();
        let response2 = handler.handle_message(msg2).await.unwrap();

        let session_id_1: String = response1
            .strip_prefix("[Session ")
            .unwrap()
            .split(']')
            .next()
            .unwrap()
            .to_string();
        let session_id_2: String = response2
            .strip_prefix("[Session ")
            .unwrap()
            .split(']')
            .next()
            .unwrap()
            .to_string();

        assert_ne!(session_id_1, session_id_2);
    }

    #[tokio::test]
    async fn test_handle_command_status() {
        let router = Arc::new(SessionRouter::new());
        let handler = AgentMessageHandler::new(router);

        let msg = make_message(crate::Platform::Telegram, "user1", "!status");
        let response = handler
            .handle_command(ManagementCommand::Status, &msg)
            .await
            .unwrap();

        assert_eq!(response, "Agent status: running");
    }

    #[tokio::test]
    async fn test_handle_command_tasks() {
        let router = Arc::new(SessionRouter::new());
        let handler = AgentMessageHandler::new(router);

        let msg = make_message(crate::Platform::Telegram, "user1", "!tasks");
        let response = handler
            .handle_command(ManagementCommand::Tasks, &msg)
            .await
            .unwrap();

        assert_eq!(response, "No scheduled tasks");
    }

    #[tokio::test]
    async fn test_handle_command_mode_with_value() {
        let router = Arc::new(SessionRouter::new());
        let handler = AgentMessageHandler::new(router);

        let msg = make_message(crate::Platform::Telegram, "user1", "!mode coding");
        let response = handler
            .handle_command(ManagementCommand::Mode(Some("coding".to_string())), &msg)
            .await
            .unwrap();

        assert_eq!(response, "Mode: coding");
    }

    #[tokio::test]
    async fn test_handle_command_mode_without_value() {
        let router = Arc::new(SessionRouter::new());
        let handler = AgentMessageHandler::new(router);

        let msg = make_message(crate::Platform::Telegram, "user1", "!mode");
        let response = handler
            .handle_command(ManagementCommand::Mode(None), &msg)
            .await
            .unwrap();

        assert_eq!(response, "Mode: general");
    }

    #[tokio::test]
    async fn test_handle_command_config_set() {
        let router = Arc::new(SessionRouter::new());
        let handler = AgentMessageHandler::new(router);

        let msg = make_message(crate::Platform::Telegram, "user1", "!config key val");
        let response = handler
            .handle_command(
                ManagementCommand::Config(Some("key".to_string()), Some("val".to_string())),
                &msg,
            )
            .await
            .unwrap();

        assert_eq!(response, "Config set: key = val");
    }

    #[tokio::test]
    async fn test_handle_command_config_get() {
        let router = Arc::new(SessionRouter::new());
        let handler = AgentMessageHandler::new(router);

        let msg = make_message(crate::Platform::Telegram, "user1", "!config key");
        let response = handler
            .handle_command(
                ManagementCommand::Config(Some("key".to_string()), None),
                &msg,
            )
            .await
            .unwrap();

        assert_eq!(response, "Config get: key");
    }
}
