//! Telegram bot integration using teloxide.
//!
//! Implements the `MessagingBot` trait for Telegram, handling:
//! - Long-polling for incoming messages
//! - Management commands (/status, /tasks, /mode, /schedule, /config)
//! - Response delivery within 30 seconds (tokio timeout)
//! - Diff image delivery as photos or documents

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use teloxide::prelude::*;
use teloxide::types::InputFile;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::{
    IncomingAttachment, IncomingMessage, ManagementCommand, MessageContent, MessageHandler,
    MessageTarget, MessagingBot, MessagingError, Platform, PlatformStatus,
};

/// Maximum time allowed to process and respond to a message (30 seconds).
const DEFAULT_RESPONSE_TIMEOUT_SECS: u64 = 30;

/// Download the largest photo and/or document attached to a Telegram message.
/// Returns the downloaded files as `IncomingAttachment`s (best-effort — failures
/// are logged and skipped rather than aborting message handling).
async fn download_telegram_attachments(
    bot: &Bot,
    token: &str,
    msg: &Message,
) -> Vec<IncomingAttachment> {
    let mut out = Vec::new();

    // Photos arrive as multiple resolutions; the last entry is the largest.
    if let Some(largest) = msg.photo().and_then(|sizes| sizes.last()) {
        if let Some(att) = fetch_telegram_file(bot, token, &largest.file.id, None).await {
            out.push(att);
        }
    }

    // Documents carry their original filename.
    if let Some(doc) = msg.document() {
        if let Some(att) =
            fetch_telegram_file(bot, token, &doc.file.id, doc.file_name.as_deref()).await
        {
            out.push(att);
        }
    }

    out
}

/// Resolve a Telegram file_id to its bytes via getFile + the file download URL.
async fn fetch_telegram_file(
    bot: &Bot,
    token: &str,
    file_id: &str,
    preferred_name: Option<&str>,
) -> Option<IncomingAttachment> {
    let file = match bot.get_file(file_id.to_string()).await {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "Telegram getFile failed");
            return None;
        }
    };
    let url = format!("https://api.telegram.org/file/bot{token}/{}", file.path);
    let resp = tokio::time::timeout(std::time::Duration::from_secs(30), reqwest::get(&url))
        .await
        .map_err(|_| {
            warn!("Telegram file download timed out");
        })
        .ok()?
        .ok()?;
    if !resp.status().is_success() {
        warn!(status = %resp.status(), "Telegram file download failed");
        return None;
    }
    let bytes = resp.bytes().await.ok()?.to_vec();

    // Prefer the original filename (documents); fall back to the storage path's
    // basename (photos), then a generic name.
    let filename = preferred_name
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .or_else(|| {
            std::path::Path::new(&file.path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "attachment".to_string());

    Some(IncomingAttachment {
        filename,
        data: bytes,
    })
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the Telegram bot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// The bot token obtained from @BotFather.
    pub bot_token: String,

    /// List of allowed chat IDs that can interact with the bot.
    /// If empty, all chats are rejected.
    pub allowed_chat_ids: Vec<i64>,

    /// Response timeout in seconds (default: 30).
    #[serde(default = "default_response_timeout")]
    pub response_timeout_secs: u64,
}

fn default_response_timeout() -> u64 {
    DEFAULT_RESPONSE_TIMEOUT_SECS
}

// =============================================================================
// Telegram Bot
// =============================================================================

/// Telegram bot implementation using teloxide.
///
/// Connects to the Telegram Bot API via long-polling, processes incoming
/// text messages as conversation input or management commands, and responds
/// within the configured timeout (default 30 seconds).
pub struct TelegramBot {
    config: TelegramConfig,
    handler: Arc<dyn MessageHandler>,
    status: Arc<RwLock<PlatformStatus>>,
}

impl std::fmt::Debug for TelegramBot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramBot")
            .field("config", &self.config)
            .field("status", &"<RwLock<PlatformStatus>>")
            .finish()
    }
}

impl TelegramBot {
    /// Create a new Telegram bot with the given configuration and message handler.
    pub fn new(
        config: TelegramConfig,
        handler: Arc<dyn MessageHandler>,
    ) -> Result<Self, MessagingError> {
        if config.bot_token.is_empty() {
            return Err(MessagingError::Config(
                "bot_token must not be empty".to_string(),
            ));
        }
        if config.allowed_chat_ids.is_empty() {
            return Err(MessagingError::Config(
                "allowed_chat_ids must contain at least one chat ID".to_string(),
            ));
        }

        Ok(Self {
            config,
            handler,
            status: Arc::new(RwLock::new(PlatformStatus {
                platform: Platform::Telegram,
                connected: false,
                last_message_at: None,
                error: None,
            })),
        })
    }

    /// Check if a chat ID is in the allowed list.
    fn is_chat_allowed(&self, chat_id: i64) -> bool {
        self.config.allowed_chat_ids.contains(&chat_id)
    }
}

#[async_trait]
impl MessagingBot for TelegramBot {
    async fn start(&self) -> Result<(), MessagingError> {
        info!("Starting Telegram bot...");

        let bot = Bot::new(&self.config.bot_token);
        let config = self.config.clone();
        let handler = Arc::clone(&self.handler);
        let status = Arc::clone(&self.status);

        // Mark as connected
        {
            let mut s = status.write().await;
            s.connected = true;
            s.error = None;
        }

        info!(
            allowed_chats = ?config.allowed_chat_ids,
            timeout_secs = config.response_timeout_secs,
            "Telegram bot configured and polling"
        );

        // Start the long-polling loop using teloxide::repl
        let status_clone = Arc::clone(&status);
        teloxide::repl(bot, move |bot: Bot, msg: Message| {
            let handler = Arc::clone(&handler);
            let config = config.clone();
            let status = Arc::clone(&status_clone);
            async move {
                // Accept text, captions (on media), or media-only messages.
                let text = msg
                    .text()
                    .or_else(|| msg.caption())
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let has_media = msg.photo().is_some() || msg.document().is_some();
                if text.is_empty() && !has_media {
                    return Ok(());
                }

                let chat_id = msg.chat.id;

                // Authorization check
                if !config.allowed_chat_ids.contains(&chat_id.0) {
                    warn!(
                        chat_id = chat_id.0,
                        "Rejected message from unauthorized chat"
                    );
                    let _ = bot
                        .send_message(
                            chat_id,
                            "⛔ Unauthorized: Your chat is not allowed to interact with this bot.",
                        )
                        .await;
                    return Ok(());
                }

                // Update last message timestamp
                {
                    let mut s = status.write().await;
                    s.last_message_at = Some(Utc::now());
                }

                let user_id = msg
                    .from
                    .as_ref()
                    .map(|u| u.id.0.to_string())
                    .unwrap_or_default();

                // Download any attached photo/document so the agent can use it.
                let attachments =
                    download_telegram_attachments(&bot, &config.bot_token, &msg).await;

                let incoming = IncomingMessage {
                    platform: Platform::Telegram,
                    platform_user_id: user_id,
                    channel_id: Some(chat_id.0.to_string()),
                    content: text.clone(),
                    timestamp: Utc::now(),
                    attachments,
                };

                // Check if this is a management command
                let timeout = Duration::from_secs(config.response_timeout_secs);

                let response_text = if let Some(command) = ManagementCommand::parse(&text) {
                    // Process command with timeout
                    let incoming_clone = incoming.clone();
                    let result = tokio::time::timeout(timeout, async {
                        handler.handle_command(command, &incoming_clone).await
                    })
                    .await;

                    match result {
                        Ok(Ok(response)) => response,
                        Ok(Err(e)) => format!("❌ Command error: {e}"),
                        Err(_) => format!(
                            "⏱️ Command timed out after {}s",
                            config.response_timeout_secs
                        ),
                    }
                } else {
                    // Process as conversation message with timeout
                    let result = tokio::time::timeout(timeout, async {
                        handler.handle_message(incoming).await
                    })
                    .await;

                    match result {
                        Ok(Ok(response)) => response,
                        Ok(Err(e)) => format!("❌ Error: {e}"),
                        Err(_) => format!(
                            "⏱️ Response timed out after {}s",
                            config.response_timeout_secs
                        ),
                    }
                };

                // Send the response
                if let Err(e) = bot.send_message(chat_id, &response_text).await {
                    error!(error = %e, chat_id = chat_id.0, "Failed to send response");
                }

                Ok(())
            }
        })
        .await;

        // If we reach here, polling has stopped
        {
            let mut s = self.status.write().await;
            s.connected = false;
        }

        info!("Telegram bot stopped");
        Ok(())
    }

    async fn stop(&self) -> Result<(), MessagingError> {
        // teloxide::repl doesn't provide a direct shutdown handle,
        // but we update status to reflect the bot is stopping.
        let mut s = self.status.write().await;
        s.connected = false;
        info!("Telegram bot stop requested");
        Ok(())
    }

    async fn send_message(
        &self,
        target: &MessageTarget,
        content: MessageContent,
    ) -> Result<(), MessagingError> {
        let chat_id_str = target.channel_id.as_deref().unwrap_or(&target.user_id);

        let chat_id_parsed: i64 = chat_id_str
            .parse()
            .map_err(|_| MessagingError::SendFailed(format!("Invalid chat ID: {chat_id_str}")))?;

        if !self.is_chat_allowed(chat_id_parsed) {
            return Err(MessagingError::Unauthorized(format!(
                "Chat {chat_id_parsed} is not in the allowed list"
            )));
        }

        let bot = Bot::new(&self.config.bot_token);
        let telegram_chat_id = ChatId(chat_id_parsed);

        match content {
            MessageContent::Text(text) => {
                bot.send_message(telegram_chat_id, &text)
                    .await
                    .map_err(|e| MessagingError::SendFailed(format!("Telegram send error: {e}")))?;
            }
            MessageContent::Image { data, caption } => {
                send_image_to_chat(&bot, telegram_chat_id, &data, caption.as_deref()).await?;
            }
            MessageContent::TextWithImage { text, image } => {
                // Send text first, then image
                bot.send_message(telegram_chat_id, &text)
                    .await
                    .map_err(|e| MessagingError::SendFailed(format!("Telegram send error: {e}")))?;

                send_image_to_chat(&bot, telegram_chat_id, &image, None).await?;
            }
        }

        Ok(())
    }

    fn status(&self) -> PlatformStatus {
        match self.status.try_read() {
            Ok(s) => s.clone(),
            Err(_) => PlatformStatus {
                platform: Platform::Telegram,
                connected: false,
                last_message_at: None,
                error: Some("Status unavailable".into()),
            },
        }
    }

    fn platform(&self) -> Platform {
        Platform::Telegram
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Send an image to a Telegram chat.
///
/// Attempts to send as a photo first (for PNG images under 10MB).
/// Falls back to sending as a document for SVG or large files.
async fn send_image_to_chat(
    bot: &Bot,
    chat_id: ChatId,
    image_data: &[u8],
    caption: Option<&str>,
) -> Result<(), MessagingError> {
    // Determine if this is an SVG or PNG
    let is_svg = image_data.starts_with(b"<svg") || image_data.starts_with(b"<?xml");

    if !is_svg && image_data.len() < 10 * 1024 * 1024 {
        // Try sending as photo (Telegram supports up to 10MB photos)
        let photo_file = InputFile::memory(image_data.to_vec()).file_name("diff.png".to_string());
        let mut request = bot.send_photo(chat_id, photo_file);
        if let Some(cap) = caption {
            request = request.caption(cap);
        }
        match request.await {
            Ok(_) => return Ok(()),
            Err(e) => {
                warn!(error = %e, "Failed to send as photo, falling back to document");
            }
        }
    }

    // Send as document (works for SVG and large files)
    let file_name = if is_svg { "diff.svg" } else { "diff.png" };
    let doc_file = InputFile::memory(image_data.to_vec()).file_name(file_name.to_string());
    let mut request = bot.send_document(chat_id, doc_file);
    if let Some(cap) = caption {
        request = request.caption(cap);
    }
    request
        .await
        .map_err(|e| MessagingError::SendFailed(format!("Telegram document send error: {e}")))?;

    Ok(())
}

/// Convenience function to send a diff image to a Telegram chat.
///
/// Used by the coding module to deliver diff images inline with responses.
pub async fn send_diff_image(
    bot: &TelegramBot,
    chat_id: &str,
    image_data: Vec<u8>,
    caption: Option<String>,
) -> Result<(), MessagingError> {
    let target = MessageTarget {
        platform: Platform::Telegram,
        user_id: chat_id.to_string(),
        channel_id: Some(chat_id.to_string()),
    };

    let content = match caption {
        Some(cap) => MessageContent::TextWithImage {
            text: cap,
            image: image_data,
        },
        None => MessageContent::Image {
            data: image_data,
            caption: None,
        },
    };

    bot.send_message(&target, content).await
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessagingError;

    /// A mock message handler for testing.
    struct MockHandler;

    #[async_trait]
    impl MessageHandler for MockHandler {
        async fn handle_message(&self, message: IncomingMessage) -> Result<String, MessagingError> {
            Ok(format!("Echo: {}", message.content))
        }

        async fn handle_command(
            &self,
            command: ManagementCommand,
            _from: &IncomingMessage,
        ) -> Result<String, MessagingError> {
            match command {
                ManagementCommand::Status => Ok("Agent is idle".to_string()),
                ManagementCommand::Tasks => Ok("No tasks".to_string()),
                _ => Ok("Command received".to_string()),
            }
        }
    }

    fn mock_handler() -> Arc<dyn MessageHandler> {
        Arc::new(MockHandler)
    }

    #[test]
    fn test_config_validation_empty_token() {
        let config = TelegramConfig {
            bot_token: String::new(),
            allowed_chat_ids: vec![123],
            response_timeout_secs: 30,
        };
        let result = TelegramBot::new(config, mock_handler());
        assert!(result.is_err());
        match result.unwrap_err() {
            MessagingError::Config(msg) => assert!(msg.contains("bot_token")),
            _ => panic!("Expected Config error"),
        }
    }

    #[test]
    fn test_config_validation_empty_chat_ids() {
        let config = TelegramConfig {
            bot_token: "test-token".to_string(),
            allowed_chat_ids: vec![],
            response_timeout_secs: 30,
        };
        let result = TelegramBot::new(config, mock_handler());
        assert!(result.is_err());
        match result.unwrap_err() {
            MessagingError::Config(msg) => assert!(msg.contains("allowed_chat_ids")),
            _ => panic!("Expected Config error"),
        }
    }

    #[test]
    fn test_config_validation_valid() {
        let config = TelegramConfig {
            bot_token: "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11".to_string(),
            allowed_chat_ids: vec![123456789, -100123456789],
            response_timeout_secs: 30,
        };
        let result = TelegramBot::new(config, mock_handler());
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_chat_allowed() {
        let config = TelegramConfig {
            bot_token: "token".to_string(),
            allowed_chat_ids: vec![100, 200, 300],
            response_timeout_secs: 30,
        };
        let bot = TelegramBot::new(config, mock_handler()).unwrap();
        assert!(bot.is_chat_allowed(100));
        assert!(bot.is_chat_allowed(200));
        assert!(bot.is_chat_allowed(300));
        assert!(!bot.is_chat_allowed(999));
        assert!(!bot.is_chat_allowed(-1));
    }

    #[test]
    fn test_response_timeout_default() {
        let config = TelegramConfig {
            bot_token: "token".to_string(),
            allowed_chat_ids: vec![100],
            response_timeout_secs: 30,
        };
        let bot = TelegramBot::new(config, mock_handler()).unwrap();
        assert_eq!(bot.config.response_timeout_secs, 30);
    }

    #[test]
    fn test_response_timeout_custom() {
        let config = TelegramConfig {
            bot_token: "token".to_string(),
            allowed_chat_ids: vec![100],
            response_timeout_secs: 15,
        };
        let bot = TelegramBot::new(config, mock_handler()).unwrap();
        assert_eq!(bot.config.response_timeout_secs, 15);
    }

    #[test]
    fn test_platform_returns_telegram() {
        let config = TelegramConfig {
            bot_token: "token".to_string(),
            allowed_chat_ids: vec![100],
            response_timeout_secs: 30,
        };
        let bot = TelegramBot::new(config, mock_handler()).unwrap();
        assert_eq!(bot.platform(), Platform::Telegram);
    }

    #[test]
    fn test_initial_status_disconnected() {
        let config = TelegramConfig {
            bot_token: "token".to_string(),
            allowed_chat_ids: vec![100],
            response_timeout_secs: 30,
        };
        let bot = TelegramBot::new(config, mock_handler()).unwrap();
        let status = bot.status();
        assert_eq!(status.platform, Platform::Telegram);
        assert!(!status.connected);
        assert!(status.last_message_at.is_none());
        assert!(status.error.is_none());
    }

    #[test]
    fn test_config_deserialization_with_defaults() {
        let json = r#"{"bot_token": "test-token", "allowed_chat_ids": [123]}"#;
        let config: TelegramConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.bot_token, "test-token");
        assert_eq!(config.allowed_chat_ids, vec![123]);
        assert_eq!(config.response_timeout_secs, 30);
    }

    #[test]
    fn test_config_deserialization_with_custom_timeout() {
        let json = r#"{"bot_token": "test-token", "allowed_chat_ids": [123, 456], "response_timeout_secs": 15}"#;
        let config: TelegramConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.response_timeout_secs, 15);
        assert_eq!(config.allowed_chat_ids, vec![123, 456]);
    }

    #[test]
    fn test_config_deserialization_negative_chat_ids() {
        // Telegram group chats have negative IDs
        let json = r#"{"bot_token": "token", "allowed_chat_ids": [-100123456789, 987654321]}"#;
        let config: TelegramConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.allowed_chat_ids, vec![-100123456789, 987654321]);
    }

    #[tokio::test]
    async fn test_send_message_invalid_chat_id() {
        let config = TelegramConfig {
            bot_token: "token".to_string(),
            allowed_chat_ids: vec![100],
            response_timeout_secs: 30,
        };
        let bot = TelegramBot::new(config, mock_handler()).unwrap();

        let target = MessageTarget {
            platform: Platform::Telegram,
            user_id: "not_a_number".to_string(),
            channel_id: Some("not_a_number".to_string()),
        };

        let result = bot
            .send_message(&target, MessageContent::Text("hello".to_string()))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_message_unauthorized_chat() {
        let config = TelegramConfig {
            bot_token: "token".to_string(),
            allowed_chat_ids: vec![100],
            response_timeout_secs: 30,
        };
        let bot = TelegramBot::new(config, mock_handler()).unwrap();

        let target = MessageTarget {
            platform: Platform::Telegram,
            user_id: "999".to_string(),
            channel_id: Some("999".to_string()),
        };

        let result = bot
            .send_message(&target, MessageContent::Text("hello".to_string()))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            MessagingError::Unauthorized(msg) => assert!(msg.contains("999")),
            other => panic!("Expected Unauthorized error, got: {other:?}"),
        }
    }

    #[test]
    fn test_management_command_parse_status() {
        // The lib.rs ManagementCommand uses '!' prefix
        let cmd = ManagementCommand::parse("!status");
        assert_eq!(cmd, Some(ManagementCommand::Status));
    }

    #[test]
    fn test_management_command_parse_tasks() {
        let cmd = ManagementCommand::parse("!tasks");
        assert_eq!(cmd, Some(ManagementCommand::Tasks));
    }

    #[test]
    fn test_management_command_parse_mode() {
        let cmd = ManagementCommand::parse("!mode coding");
        assert_eq!(
            cmd,
            Some(ManagementCommand::Mode(Some("coding".to_string())))
        );
    }

    #[test]
    fn test_management_command_non_command() {
        let cmd = ManagementCommand::parse("hello world");
        assert_eq!(cmd, None);
    }
}
