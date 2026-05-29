//! Agent message handler for messaging platform integration.
//!
//! Routes incoming messages through the session router to ensure each
//! platform user gets their own isolated session with separate history
//! and context.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::session_router::SessionRouter;
use crate::{IncomingMessage, ManagementCommand, MessageHandler, MessagingError};

/// A MessageHandler that routes messages through the session router.
///
/// Each platform user gets their own isolated session. In a full implementation,
/// this would hold an `Arc<AgentCore>` and route messages through the agent's
/// processing pipeline with the appropriate session context. For now the agent
/// dispatch is stubbed (echo), but inbound attachments ARE downloaded and saved
/// to `{workspace}/uploads/{session_id}/` so the plumbing is in place.
pub struct AgentMessageHandler {
    session_router: Arc<SessionRouter>,
    /// Workspace root for storing uploaded attachments. When `None`, attachments
    /// are acknowledged but not written to disk.
    workspace_dir: Option<PathBuf>,
    // In a full implementation, this would also hold an Arc<AgentCore>.
}

impl AgentMessageHandler {
    /// Create a new AgentMessageHandler with the given session router.
    pub fn new(session_router: Arc<SessionRouter>) -> Self {
        Self {
            session_router,
            workspace_dir: None,
        }
    }

    /// Set the workspace directory used to persist inbound attachments.
    pub fn with_workspace_dir(mut self, dir: PathBuf) -> Self {
        self.workspace_dir = Some(dir);
        self
    }

    /// Get a reference to the underlying session router.
    pub fn session_router(&self) -> &Arc<SessionRouter> {
        &self.session_router
    }

    /// Persist a message's attachments under `{workspace}/uploads/{session_id}/`.
    /// Returns the workspace-relative paths of stored files.
    async fn save_attachments(&self, session_id: &str, message: &IncomingMessage) -> Vec<String> {
        if message.attachments.is_empty() {
            return Vec::new();
        }
        let Some(workspace) = &self.workspace_dir else {
            warn!("No workspace configured; skipping attachment save");
            return Vec::new();
        };

        let dir = common::uploads::session_upload_dir(workspace, session_id);
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            warn!(error = %e, "Failed to create upload dir for messaging attachment");
            return Vec::new();
        }

        let mut saved = Vec::new();
        for att in &message.attachments {
            let filename = common::uploads::sanitize_filename(&att.filename);
            let dest = dir.join(&filename);
            match tokio::fs::write(&dest, &att.data).await {
                Ok(()) => saved.push(common::uploads::display_path(session_id, &filename)),
                Err(e) => warn!(error = %e, file = %filename, "Failed to save attachment"),
            }
        }
        saved
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
            attachments = message.attachments.len(),
            "Routing message through session"
        );

        // 2. Persist any attachments to the session's upload directory.
        let saved = self
            .save_attachments(&session_id.to_string(), &message)
            .await;

        // 3. Route message through agent core with this session.
        // For now, return a placeholder indicating the session was found/created.
        // In a full implementation, this would call:
        //   self.agent_core.process_message(session_id, message, history).await
        if saved.is_empty() {
            Ok(format!(
                "[Session {}] Received: {}",
                session_id, message.content
            ))
        } else {
            Ok(format!(
                "[Session {}] Received: {} (saved {} file(s): {})",
                session_id,
                message.content,
                saved.len(),
                saved.join(", ")
            ))
        }
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
            attachments: Vec::new(),
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
