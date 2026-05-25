//! Messaging Integration — bridges third-party messaging platforms to the agent.
//!
//! Responsibilities:
//! - Telegram bot integration (teloxide)
//! - Discord bot integration (serenity)
//! - WhatsApp bot integration (whatsapp-web-rs)
//! - Identity mapping and authorization enforcement

pub mod agent_handler;
pub mod discord;
pub mod identity;
pub mod session_router;
pub mod telegram;
// NOTE: whatsapp module is not yet compilable (task 23.3 pending)
// pub mod whatsapp;

pub use agent_handler::AgentMessageHandler;
pub use session_router::SessionRouter;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur in messaging integrations.
#[derive(Debug, Error)]
pub enum MessagingError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    #[error("send failed: {0}")]
    SendFailed(String),

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("timeout: response not sent within deadline")]
    Timeout,

    #[error("unauthorized user: {0}")]
    Unauthorized(String),

    #[error("platform error: {0}")]
    Platform(String),

    #[error("configuration error: {0}")]
    Config(String),
}

/// Content that can be sent to a messaging platform.
#[derive(Debug, Clone)]
pub enum MessageContent {
    /// Plain text message.
    Text(String),
    /// Image with optional caption.
    Image {
        data: Vec<u8>,
        caption: Option<String>,
    },
    /// Text message with an attached image (e.g., diff image).
    TextWithImage { text: String, image: Vec<u8> },
}

/// Target for sending a message.
#[derive(Debug, Clone)]
pub struct MessageTarget {
    pub platform: Platform,
    pub user_id: String,
    pub channel_id: Option<String>,
}

/// Supported messaging platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    Telegram,
    Discord,
    WhatsApp,
}

/// Status of a platform connection.
#[derive(Debug, Clone)]
pub struct PlatformStatus {
    pub platform: Platform,
    pub connected: bool,
    pub last_message_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

/// A file attached to an incoming message (downloaded from the platform).
#[derive(Debug, Clone)]
pub struct IncomingAttachment {
    /// Suggested filename (sanitized before being written to disk).
    pub filename: String,
    /// Raw file bytes downloaded from the platform.
    pub data: Vec<u8>,
}

/// Result of processing an incoming message.
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub platform: Platform,
    pub platform_user_id: String,
    pub channel_id: Option<String>,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Files attached to the message (photos, documents). Empty for text-only.
    pub attachments: Vec<IncomingAttachment>,
}

/// Management command parsed from a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagementCommand {
    /// !status — query agent status
    Status,
    /// !tasks — list scheduled tasks
    Tasks,
    /// !mode [general|coding] — switch agent mode
    Mode(Option<String>),
    /// !schedule <cron> <action> — schedule a task
    Schedule(String),
    /// !config [key] [value] — view or update config
    Config(Option<String>, Option<String>),
}

impl ManagementCommand {
    /// Parse a management command from a message string.
    /// Returns None if the message is not a command (doesn't start with !).
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if !trimmed.starts_with('!') {
            return None;
        }

        let parts: Vec<&str> = trimmed[1..].splitn(3, ' ').collect();
        match parts.first().map(|s| s.to_lowercase()).as_deref() {
            Some("status") => Some(ManagementCommand::Status),
            Some("tasks") => Some(ManagementCommand::Tasks),
            Some("mode") => {
                let mode = parts.get(1).map(|s| s.to_string());
                Some(ManagementCommand::Mode(mode))
            }
            Some("schedule") => {
                let rest = if parts.len() > 1 {
                    trimmed[1..]
                        .trim_start_matches("schedule")
                        .trim()
                        .to_string()
                } else {
                    String::new()
                };
                Some(ManagementCommand::Schedule(rest))
            }
            Some("config") => {
                let key = parts.get(1).map(|s| s.to_string());
                let value = parts.get(2).map(|s| s.to_string());
                Some(ManagementCommand::Config(key, value))
            }
            _ => None,
        }
    }
}

/// Trait that all messaging bot implementations must satisfy.
#[async_trait]
pub trait MessagingBot: Send + Sync {
    /// Start the bot and connect to the platform gateway.
    async fn start(&self) -> Result<(), MessagingError>;

    /// Stop the bot gracefully.
    async fn stop(&self) -> Result<(), MessagingError>;

    /// Send a message to a target user/channel.
    async fn send_message(
        &self,
        target: &MessageTarget,
        content: MessageContent,
    ) -> Result<(), MessagingError>;

    /// Get the current connection status.
    fn status(&self) -> PlatformStatus;

    /// Get the platform this bot serves.
    fn platform(&self) -> Platform;
}

/// Callback trait for handling incoming messages from any platform.
/// Implementations route messages to the Agent Core for processing.
#[async_trait]
pub trait MessageHandler: Send + Sync {
    /// Handle an incoming conversation message. Returns the response text.
    async fn handle_message(&self, message: IncomingMessage) -> Result<String, MessagingError>;

    /// Handle a management command. Returns the command result text.
    async fn handle_command(
        &self,
        command: ManagementCommand,
        from: &IncomingMessage,
    ) -> Result<String, MessagingError>;
}
